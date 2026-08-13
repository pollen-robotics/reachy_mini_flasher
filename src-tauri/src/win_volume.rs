//! Windows: take the target disk away from the filesystem stack before a raw
//! image write.
//!
//! Unlike macOS (`diskutil unmountDisk`), Windows has no single "unmount this
//! disk" call, and writing to `\\.\PhysicalDriveN` while its volumes are
//! mounted does **not** cleanly fail: the first sectors usually go through and
//! the write then dies with a sharing violation partway in, or the mounted
//! filesystem writes its own metadata over what we just wrote. The exposed CM4
//! eMMC has partitions Windows auto-mounts, so this path is always hit on real
//! hardware.
//!
//! The sequence below is what balenaEtcher's `etcher-sdk` does, and what the
//! Win32 docs prescribe for raw disk writers:
//!
//!   1. find every volume that lives on the target physical drive,
//!   2. `FSCTL_LOCK_VOLUME` each one (fails while any handle is open on it, so
//!      it is retried for a couple of seconds),
//!   3. `FSCTL_DISMOUNT_VOLUME` each one,
//!   4. **keep the handles open** for the whole write - closing one releases the
//!      lock and lets Windows remount the volume mid-flash,
//!   5. on drop: release the locks and `IOCTL_DISK_UPDATE_PROPERTIES` so
//!      Explorer re-reads the freshly written partition table.
//!
//! Note what is NOT done: deleting the partition table up front. It voids the
//! locks we just took - see `lock_disk`.
//!
//! `lock_disk()` returns a guard; hold it until the write is finished.

use std::ffi::c_void;
use std::thread::sleep;
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_NO_MORE_FILES, GENERIC_READ, GENERIC_WRITE, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, FILE_ATTRIBUTE_NORMAL,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, IOCTL_DISK_UPDATE_PROPERTIES,
    IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER,
};
use windows::Win32::System::IO::DeviceIoControl;

/// `FSCTL_LOCK_VOLUME` fails while anything still holds a handle on the volume
/// (Explorer previewing it, an indexer, an antivirus scan of the freshly
/// appeared disk...). Those settle within a second or two after the eMMC
/// enumerates, so retry rather than failing the flash outright.
const LOCK_ATTEMPTS: u32 = 12;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Handle wrapper
// ---------------------------------------------------------------------------

/// Owns a Win32 `HANDLE` and closes it on drop.
///
/// For locked volumes the close is the whole point: it is what releases
/// `FSCTL_LOCK_VOLUME`, so these must outlive the write.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// SAFETY: a raw disk/volume HANDLE is just a kernel object index; it carries no
// thread affinity and the flash runs it on a worker thread.
unsafe impl Send for OwnedHandle {}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Access mask for merely *identifying* a device.
///
/// `dwDesiredAccess = 0` grants no read or write rights but still allows the
/// query ioctls, and - crucially - succeeds on volumes that would refuse a
/// read/write open. Identifying a volume must never require the right to
/// modify it, or the ones we most need to find are the ones we skip.
const ACCESS_QUERY: u32 = 0;
/// Access mask required to lock and dismount a volume.
const ACCESS_RW: u32 = GENERIC_READ.0 | GENERIC_WRITE.0;

/// Open a device path (`\\.\PhysicalDriveN`, `\\?\Volume{...}`) for raw access.
fn open_device(path: &str, access: u32) -> Result<OwnedHandle, String> {
    let wpath = wide(path);
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wpath.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map_err(|e| format!("failed to open {path}: {e}"))?;
    Ok(OwnedHandle(handle))
}

/// Issue an ioctl that takes and returns no payload (the FSCTL_* volume ones).
fn ioctl_void(handle: &OwnedHandle, code: u32) -> Result<(), String> {
    let mut returned: u32 = 0;
    unsafe { DeviceIoControl(handle.0, code, None, 0, None, 0, Some(&mut returned), None) }
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Disk / volume discovery
// ---------------------------------------------------------------------------

/// `\\.\PhysicalDrive3` -> `3`.
pub fn disk_number_from_target(target: &str) -> Option<u32> {
    let lower = target.to_lowercase().replace('/', "\\");
    let idx = lower.find("physicaldrive")?;
    lower[idx + "physicaldrive".len()..]
        .trim_end_matches('\\')
        .parse()
        .ok()
}

/// Physical drive number backing an open volume handle, via
/// `IOCTL_STORAGE_GET_DEVICE_NUMBER`.
fn volume_disk_number(handle: &OwnedHandle) -> Option<u32> {
    let mut info = STORAGE_DEVICE_NUMBER::default();
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle.0,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            None,
            0,
            Some(&mut info as *mut _ as *mut c_void),
            std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32,
            Some(&mut returned),
            None,
        )
    };
    // Fails for volumes that aren't backed by a single physical disk (spanned,
    // storage-space, virtual). Those are never our target, so skipping is right.
    ok.ok().map(|()| info.DeviceNumber)
}

/// Every volume GUID path on the system, as `\\?\Volume{...}` (no trailing `\`).
///
/// Volume GUID paths are used rather than drive letters on purpose: partitions
/// on the CM4 eMMC (the Linux rootfs, notably) get no letter at all, but they
/// are still mounted enough to fight a raw write.
fn all_volumes() -> Result<Vec<String>, String> {
    let mut buf = [0u16; 260];
    let find = unsafe { FindFirstVolumeW(&mut buf) }
        .map_err(|e| format!("failed to enumerate volumes: {e}"))?;

    let mut out = Vec::new();
    loop {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let name = String::from_utf16_lossy(&buf[..end]);
        if !name.is_empty() {
            // FindFirstVolume/FindNextVolume yield a trailing backslash, which
            // CreateFileW rejects for a device open.
            out.push(name.trim_end_matches('\\').to_string());
        }

        buf = [0u16; 260];
        if unsafe { FindNextVolumeW(find, &mut buf) }.is_err() {
            // ERROR_NO_MORE_FILES is the normal end of enumeration.
            let last = unsafe { GetLastError() };
            if last != ERROR_NO_MORE_FILES {
                crate::flash::log(&format!("volume enumeration stopped early: {last:?}"));
            }
            break;
        }
    }

    unsafe {
        let _ = FindVolumeClose(find);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The guard
// ---------------------------------------------------------------------------

/// Holds the locked/dismounted volumes of a physical drive. Dropping it
/// releases every lock and asks Windows to re-read the partition table.
pub struct DiskLock {
    disk_number: u32,
    /// Kept alive purely for their side effect: closing releases the lock.
    _volumes: Vec<OwnedHandle>,
}

impl Drop for DiskLock {
    fn drop(&mut self) {
        // Locks are released by closing the volume handles (`_volumes`), which
        // happens right after this. Then nudge Windows into re-reading the
        // partition table we just wrote, so the robot's boot partition shows up
        // instead of the stale (deleted) layout.
        let path = format!("\\\\.\\PhysicalDrive{}", self.disk_number);
        if let Ok(disk) = open_device(&path, ACCESS_RW) {
            if let Err(e) = ioctl_void(&disk, IOCTL_DISK_UPDATE_PROPERTIES) {
                crate::flash::log(&format!("IOCTL_DISK_UPDATE_PROPERTIES on {path} failed: {e}"));
            }
        }
    }
}

/// Lock + dismount every volume of the target physical drive.
///
/// Hold the returned guard for the entire write. Returns `Ok(None)` when the
/// target isn't a physical drive path (simulation mode writes to a plain file).
pub fn lock_disk(target: &str) -> Result<Option<DiskLock>, String> {
    let Some(disk_number) = disk_number_from_target(target) else {
        return Ok(None);
    };

    let mut locked = Vec::new();
    for volume in all_volumes()? {
        // Identify with a query-only handle. Opening every volume on the system
        // read/write just to read its disk number is both rude and unreliable:
        // the system volume and various pseudo-volumes refuse it, and silently
        // skipping whatever refuses means silently skipping the robot's own
        // volumes too if they ever do - which is the one case that must not be
        // skipped, because leaving one mounted makes Windows deny the writes
        // partway into the flash.
        let probe = match open_device(&volume, ACCESS_QUERY) {
            Ok(p) => p,
            Err(e) => {
                crate::flash::log(&format!("skip {volume}: cannot open to identify ({e})"));
                continue;
            }
        };
        let on_disk = volume_disk_number(&probe);
        crate::flash::log(&format!("volume {volume} -> disk {on_disk:?}"));
        if on_disk != Some(disk_number) {
            continue;
        }
        drop(probe);

        // On the target disk. From here every failure is fatal: a volume we
        // can't lock is a volume that stays mounted, and the flash would fail
        // mid-write with a confusing ERROR_ACCESS_DENIED instead of a clear
        // message now.
        let handle = open_device(&volume, ACCESS_RW).map_err(|e| {
            format!("Could not open {volume} on the robot's storage: {e}. Close any window browsing that drive and try again.")
        })?;
        lock_and_dismount(&handle, &volume)?;
        locked.push(handle);
    }

    // Zero volumes is legitimate (a disk with no recognizable filesystem), but
    // it is also exactly what a broken enumeration looks like, so say which.
    crate::flash::log(&format!("locked {} volume(s) on PhysicalDrive{disk_number}", locked.len()));

    // NOTE: deliberately NO `IOCTL_DISK_DELETE_DRIVE_LAYOUT` here.
    //
    // Wiping the partition table looks tidy - nothing left for Windows to
    // auto-mount - and balenaEtcher does it. But it makes Windows re-read the
    // layout, which tears down the volume objects and builds fresh ones. Our
    // locks are held against the *old* objects, so they are silently voided:
    // the replacement volume is mounted, unlocked, and blocks the very write we
    // just prepared for. Observed on real hardware as
    //
    //     locked 1 volume(s) on PhysicalDrive1
    //     ERR failed to flush: Access is denied. (os error 5)
    //
    // Locking and dismounting is the documented requirement and is sufficient
    // on its own; the dismounted volumes cannot remount while we hold their
    // handles. The image overwrites the partition table anyway, and the drop
    // handler asks Windows to re-read it afterwards.

    Ok(Some(DiskLock { disk_number, _volumes: locked }))
}

fn lock_and_dismount(handle: &OwnedHandle, volume: &str) -> Result<(), String> {
    let mut last_err = String::new();
    let mut locked = false;
    for attempt in 0..LOCK_ATTEMPTS {
        match ioctl_void(handle, FSCTL_LOCK_VOLUME) {
            Ok(()) => {
                locked = true;
                break;
            }
            Err(e) => {
                last_err = e;
                if attempt + 1 < LOCK_ATTEMPTS {
                    sleep(LOCK_RETRY_DELAY);
                }
            }
        }
    }

    // Dismount even if the lock never came through: it still tears down the
    // mounted filesystem, which is the part that would corrupt the write. The
    // volume can be remounted under us without the lock, but a dismounted +
    // layout-deleted disk gives Windows no reason to.
    let dismounted = ioctl_void(handle, FSCTL_DISMOUNT_VOLUME);

    match (locked, dismounted) {
        (true, Ok(())) => Ok(()),
        (false, Ok(())) => {
            crate::flash::log(&format!("{volume}: dismounted but could not be locked ({last_err})"));
            Ok(())
        }
        (_, Err(e)) => Err(format!(
            "Windows would not release {volume} (on the robot's storage): {e}. \
             Close any window browsing that drive and try again."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::disk_number_from_target;

    #[test]
    fn parses_physical_drive_paths() {
        assert_eq!(disk_number_from_target("\\\\.\\PhysicalDrive0"), Some(0));
        assert_eq!(disk_number_from_target("\\\\.\\PhysicalDrive12"), Some(12));
        // Case-insensitive: the path is built by us, but a user override via
        // env/CLI may not match our casing.
        assert_eq!(disk_number_from_target("\\\\.\\physicaldrive3"), Some(3));
    }

    #[test]
    fn ignores_non_disk_targets() {
        // Simulation mode flashes to a temp file - no volumes to lock.
        assert_eq!(disk_number_from_target("C:\\Users\\me\\reachy-sim.img"), None);
        assert_eq!(disk_number_from_target("/dev/rdisk4"), None);
        assert_eq!(disk_number_from_target("\\\\.\\PhysicalDriveX"), None);
    }
}
