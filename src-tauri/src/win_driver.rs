//! WinUSB driver for the CM4 in download mode (Windows).
//!
//! On macOS a CM4 in download mode is usable the moment it enumerates. On
//! Windows it is inert until a **WinUSB driver is bound to it**: the BCM2711
//! exposes a vendor-specific interface, Windows has no in-box driver for it, so
//! `rpiboot.exe` cannot open it and the eMMC is never exposed. This is the one
//! step of the Windows flow that cannot be made silent - binding a driver is a
//! system change and always asks for consent.
//!
//! Rather than sending the user off to Zadig, we do exactly what Raspberry Pi's
//! own `rpiboot_setup.exe` does (see `win32/install_script.nsi` in
//! raspberrypi/usbboot): run **libwdi**'s `wdi-simple.exe` to bind WinUSB to the
//! device. One UAC prompt, one Windows driver-install dialog, done.
//!
//! Upstream binds all four Raspberry Pi boot PIDs unconditionally; we bind the
//! PID of the board that is actually plugged in, so the user still sees a single
//! prompt - and so a board other than the CM4 in a Reachy Mini Wireless gets a
//! working driver rather than a button that can never succeed.
//!
//! `wdi-simple.exe` ships in the RPiBoot installer's `redist/` folder, so it is
//! resolved from (in order) an env override, our bundled resources, and an
//! existing RPiBoot installation. When none of those has it, `can_install` is
//! false and the UI falls back to pointing at the official installer.

use serde::Serialize;
use tauri::AppHandle;

/// Official RPiBoot installer - the fallback when we can't bind the driver
/// ourselves. It installs the same WinUSB driver (plus rpiboot itself).
pub const RPIBOOT_INSTALLER_URL: &str =
    "https://github.com/raspberrypi/usbboot/raw/master/win32/rpiboot_setup.exe";

/// What the UI needs to decide whether to nag about the driver.
#[derive(Serialize, Clone, Debug)]
pub struct DriverStatus {
    /// False everywhere except Windows; the UI hides the whole concern then.
    pub applicable: bool,
    /// A Raspberry Pi boot-mode device is present in the PnP tree.
    pub device_present: bool,
    /// WinUSB (or libusb) is bound to it, so rpiboot can talk to it.
    pub driver_ok: bool,
    /// We have `wdi-simple.exe` and can bind the driver in-app.
    pub can_install: bool,
    /// Where to send the user when `can_install` is false.
    pub installer_url: &'static str,
    /// Diagnostic detail, surfaced in the dev console rather than the UI.
    pub detail: String,
}

#[cfg(target_os = "windows")]
impl DriverStatus {
    /// True when the robot is plugged in but unusable until the driver is bound.
    pub fn needs_driver(&self) -> bool {
        self.applicable && self.device_present && !self.driver_ok
    }
}

/// `async` for the same reason as `detect::detect_reachy`: a synchronous Tauri
/// command runs on the main thread, this one spawns `Get-PnpDevice`, and the
/// connect screen polls it every 4s - including all the way through the driver
/// install, which is exactly when the UI must keep repainting.
#[tauri::command]
pub async fn winusb_status(app: AppHandle) -> DriverStatus {
    tauri::async_runtime::spawn_blocking(move || platform::status(&app))
        .await
        // Same shape the Windows path uses for a failed query: report nothing
        // rather than block the flow on it.
        .unwrap_or_else(|e| DriverStatus {
            applicable: cfg!(target_os = "windows"),
            device_present: false,
            driver_ok: false,
            can_install: false,
            installer_url: RPIBOOT_INSTALLER_URL,
            detail: format!("driver status task failed: {e}"),
        })
}

#[tauri::command]
pub async fn install_winusb_driver(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || platform::install(&app))
        .await
        .map_err(|e| format!("driver install task panicked: {e}"))?
}

/// Reason the flash can't proceed yet because of the driver, if any.
///
/// Used by `rpiboot::prepare_reachy` so a driverless device produces an
/// actionable message instead of a bare "rpiboot failed".
#[cfg(target_os = "windows")]
pub fn blocking_reason(app: &AppHandle) -> Option<String> {
    let status = platform::status(app);
    if !status.needs_driver() {
        return None;
    }
    Some(
        "The USB driver for the robot's board isn't installed yet, so Windows won't let \
         the app talk to it."
            .to_string(),
    )
}

/// Is a Raspberry Pi boot-mode device present, regardless of its driver?
///
/// `nusb` only sees devices it can open, i.e. WinUSB-bound ones, so on Windows
/// download-mode detection needs this PnP-level check as well - otherwise a
/// driverless robot is indistinguishable from no robot at all.
pub fn device_present() -> bool {
    platform::device_present()
}

/// Install directory of Raspberry Pi's RPiBoot, when the user has it.
///
/// It ships `rpiboot.exe` and `mass-storage-gadget64/` alongside the driver, so
/// `rpiboot` can reuse them - which makes the flasher work on a machine that ran
/// the official installer even before we bundle our own copies.
#[cfg(target_os = "windows")]
pub fn rpiboot_install_dir() -> Option<std::path::PathBuf> {
    platform::rpiboot_install_dir()
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod platform {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use tauri::{AppHandle, Manager};

    use super::{DriverStatus, RPIBOOT_INSTALLER_URL};
    use crate::win_ps;

    /// Broadcom, used by every Raspberry Pi SoC in USB boot/download mode.
    const BROADCOM_VID: u16 = 0x0a5c;
    /// BCM2711 - the CM4 inside a Reachy Mini Wireless.
    const CM4_PID: u16 = 0x2711;

    /// Name the driver is registered under. Matches the RPiBoot installer's, so
    /// installing either one satisfies the other.
    const DRIVER_NAME: &str = "Raspberry Pi USB boot";

    #[derive(serde::Deserialize, Clone)]
    struct PnpDevice {
        #[serde(rename = "InstanceId")]
        instance_id: String,
        #[serde(rename = "Status")]
        status: String,
        #[serde(rename = "Service")]
        service: Option<String>,
    }

    /// Every present Raspberry Pi boot-mode device, driver bound or not.
    ///
    /// `PID_27*` covers the four boot PIDs (2763/2764/2711/2712); we report on
    /// any of them so a user with the wrong board still gets a real message.
    fn pnp_devices() -> Result<Vec<PnpDevice>, String> {
        let script = "Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue | \
             Where-Object { $_.InstanceId -like 'USB\\VID_0A5C&PID_27*' } | \
             ForEach-Object { [PSCustomObject]@{ InstanceId = [string]$_.InstanceId; \
             Status = [string]$_.Status; Service = [string]$_.Service } } | \
             ConvertTo-Json -Compress";
        let out = win_ps::run(script, "Get-PnpDevice")?;
        win_ps::json_array(&out, "Get-PnpDevice")
    }

    /// USB product id out of a PnP instance id
    /// (`USB\VID_0A5C&PID_2711\6&...` -> `0x2711`).
    fn instance_pid(instance_id: &str) -> Option<u16> {
        let upper = instance_id.to_uppercase();
        let hex: String = upper.split("PID_").nth(1)?.chars().take(4).collect();
        u16::from_str_radix(&hex, 16).ok()
    }

    /// PIDs of the boot devices that are present but still driverless.
    ///
    /// Detection deliberately matches every Raspberry Pi boot PID (`PID_27*`),
    /// so binding a hardcoded 0x2711 left any other board with an "Install USB
    /// driver" button that could never turn `driver_ok` true - the UI would
    /// re-offer it forever. Bind what is actually plugged in instead.
    fn pids_needing_driver() -> Vec<u16> {
        let mut pids: Vec<u16> = cached_devices()
            .unwrap_or_default()
            .iter()
            .filter(|dev| !driver_bound(dev))
            .filter_map(|dev| instance_pid(&dev.instance_id))
            .collect();
        pids.sort_unstable();
        pids.dedup();
        // Nothing enumerated - a failed query, or a device that went away
        // between the poll and the click. Fall back to the CM4 in a Reachy Mini
        // Wireless, which is what this used to bind unconditionally.
        if pids.is_empty() {
            pids.push(CM4_PID);
        }
        pids
    }

    /// A device is usable once a user-mode-capable driver is bound to it.
    /// WinUSB is what we (and RPiBoot) install; libusbK/libusb0 also work, and a
    /// user who ran Zadig may well have picked one of those.
    fn driver_bound(dev: &PnpDevice) -> bool {
        let service = dev.service.as_deref().unwrap_or("").to_lowercase();
        service.contains("winusb") || service.contains("libusb")
    }

    /// How long a system lookup is reused. Every one of these spawns a
    /// PowerShell process, and the connect screen polls them while it waits, so
    /// an uncached answer means a process launch several times a second. Short
    /// enough that plugging the robot in - or installing RPiBoot in another
    /// window - is still picked up on its own.
    ///
    /// Sits just above the 4s `winusb_status` poll rather than below it: at 3s
    /// every single poll missed the cache, which defeated the point. Detection
    /// latency for a freshly plugged robot grows by ~2s, which is not something
    /// a human waiting on a USB device notices.
    const CACHE_TTL: Duration = Duration::from_secs(5);

    /// A memoized lookup: when the value was computed, and what it produced.
    /// Empty until the first call.
    type Cache<T> = Mutex<Option<(Instant, T)>>;

    static PNP_CACHE: Cache<Result<Vec<PnpDevice>, String>> = Mutex::new(None);
    static INSTALL_DIR_CACHE: Cache<Option<PathBuf>> = Mutex::new(None);

    /// Memoize a lookup for `CACHE_TTL`.
    fn cached<T: Clone>(cell: &Cache<T>, compute: impl FnOnce() -> T) -> T {
        let mut slot = match cell.lock() {
            Ok(slot) => slot,
            // A poisoned cache is not worth failing detection over.
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some((at, value)) = slot.as_ref() {
            if at.elapsed() < CACHE_TTL {
                return value.clone();
            }
        }

        let value = compute();
        *slot = Some((Instant::now(), value.clone()));
        value
    }

    /// The device list, at most one PowerShell process per `CACHE_TTL`.
    ///
    /// Every caller goes through here. `status()` used to query directly, which
    /// made the connect screen's driver poll spawn a `Get-PnpDevice` every few
    /// seconds - exactly the cost the cache exists to avoid.
    fn cached_devices() -> Result<Vec<PnpDevice>, String> {
        cached(&PNP_CACHE, pnp_devices)
    }

    pub fn device_present() -> bool {
        cached_devices().map(|d| !d.is_empty()).unwrap_or(false)
    }

    pub fn status(app: &AppHandle) -> DriverStatus {
        let (device_present, driver_ok, detail) = match cached_devices() {
            Ok(devices) if devices.is_empty() => {
                (false, false, "no Raspberry Pi boot device present".to_string())
            }
            Ok(devices) => {
                let bound = devices.iter().any(driver_bound);
                let detail = devices
                    .iter()
                    .map(|d| {
                        format!(
                            "{} [status={}, service={}]",
                            d.instance_id,
                            d.status,
                            d.service.as_deref().unwrap_or("-")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                (true, bound, detail)
            }
            // Treat a failed query as "nothing to report" rather than blocking
            // the flow on it: the flash itself will produce the real error.
            Err(e) => (false, false, e),
        };

        DriverStatus {
            applicable: true,
            device_present,
            driver_ok,
            can_install: wdi_simple_bin(app).is_some(),
            installer_url: RPIBOOT_INSTALLER_URL,
            detail,
        }
    }

    pub fn install(app: &AppHandle) -> Result<(), String> {
        let bin = wdi_simple_bin(app).ok_or_else(|| {
            format!(
                "The driver installer helper (wdi-simple.exe) isn't bundled with this build. \
                 Install Raspberry Pi's RPiBoot from {RPIBOOT_INSTALLER_URL} instead, then \
                 come back."
            )
        })?;

        // wdi-simple extracts the generated driver package to a directory it
        // resolves relative to its OWN executable, not to the process working
        // directory - so `-WorkingDirectory` has no effect on it whatsoever.
        // Left to itself it writes `usb_driver/` into our installation folder
        // under Program Files, which then needs admin rights to clean up.
        //
        // That matters because libwdi refuses to reuse an existing extraction
        // directory: it calls CreateDirectory, gets ERROR_ALREADY_EXISTS and
        // fails the whole install with WDI_ERROR_ACCESS (-3). One failed run
        // therefore poisons every later attempt - the button stops working for
        // good, with nothing to show for it. Observed on real hardware.
        //
        // So: hand it an absolute, user-writable path, and clear it first.
        let base = app
            .path()
            .app_cache_dir()
            .unwrap_or_else(|_| std::env::temp_dir());
        std::fs::create_dir_all(&base)
            .map_err(|e| format!("failed to create the driver staging dir: {e}"))?;
        let workdir = base.join("winusb-driver");

        let mut failure = None;
        for pid in pids_needing_driver() {
            // Deliberately removed and NOT recreated - see above, libwdi wants
            // to create this itself and errors out if it already exists, so a
            // second PID cannot reuse what the first one left behind either.
            let _ = std::fs::remove_dir_all(&workdir);

            let args = format!(
                "-n \"{DRIVER_NAME}\" -v 0x{BROADCOM_VID:04x} -p 0x{pid:04x} -t 0 -d \"{}\"",
                win_ps::simplify(&workdir)
            );
            // Captured, not merely run: wdi-simple's console is hidden, so
            // without this its diagnosis is lost and a failed install is a bare
            // number. The full output also lands in the app log.
            let (code, output) = win_ps::run_elevated_captured(&bin, &args, "wdi-simple")?;
            match code {
                // One bound device is all `driver_ok` needs, so stop here
                // rather than raising a second UAC prompt.
                0 => return Ok(()),
                // libwdi reports its own negative error codes; the number alone
                // means little, so lead with whatever it printed and keep the
                // code for a bug report.
                code => {
                    let said = win_ps::output_tail(&output, 4);
                    let said = if said.is_empty() {
                        "it gave no reason".to_string()
                    } else {
                        said
                    };
                    failure = Some(format!(
                        "The USB driver could not be installed for USB device 0x{pid:04x}: \
                         {said} (installer error {code}). Installing Raspberry Pi's RPiBoot \
                         from {RPIBOOT_INSTALLER_URL} does the same job and is a reliable \
                         fallback."
                    ))
                }
            }
        }
        match failure {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Locate libwdi's `wdi-simple.exe`: env override -> bundled resource ->
    /// an existing RPiBoot installation.
    fn wdi_simple_bin(app: &AppHandle) -> Option<PathBuf> {
        if let Ok(p) = std::env::var("REACHY_WDI_SIMPLE_BIN") {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Some(p);
            }
        }
        if let Ok(res) = app.path().resource_dir() {
            let cand = res.join("rpiboot").join("wdi-simple.exe");
            if cand.is_file() {
                return Some(cand);
            }
        }
        let cand = rpiboot_install_dir()?.join("redist").join("wdi-simple.exe");
        cand.is_file().then_some(cand)
    }

    /// Where Raspberry Pi's RPiBoot installer put itself, if it ran.
    ///
    /// It records its install dir as the default value of
    /// `HKCU\Software\Raspberry Pi`; the well-known default is checked too, in
    /// case the key is missing (installed under another account, say).
    pub fn rpiboot_install_dir() -> Option<PathBuf> {
        cached(&INSTALL_DIR_CACHE, find_rpiboot_install_dir)
    }

    fn find_rpiboot_install_dir() -> Option<PathBuf> {
        let script = "(Get-ItemProperty -Path 'HKCU:\\Software\\Raspberry Pi' \
             -ErrorAction SilentlyContinue).'(default)'";
        if let Ok(out) = win_ps::run(script, "RPiBoot registry lookup") {
            let dir = PathBuf::from(out.trim());
            if dir.is_dir() {
                return Some(dir);
            }
        }

        ["ProgramFiles", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(|var| std::env::var(var).ok())
            .map(|base| PathBuf::from(base).join("Raspberry Pi"))
            .find(|dir| dir.is_dir())
    }

    #[cfg(test)]
    mod tests {
        use super::instance_pid;

        #[test]
        fn reads_the_pid_out_of_an_instance_id() {
            // What Get-PnpDevice returns for a CM4 in boot mode.
            assert_eq!(instance_pid(r"USB\VID_0A5C&PID_2711\6&1D0E4B2&0&2"), Some(0x2711));
            // The other Raspberry Pi boot PIDs. Detection matches these too, so
            // the install has to follow the device instead of assuming 0x2711.
            assert_eq!(instance_pid(r"USB\VID_0A5C&PID_2764\5&2A6F1B3&0&1"), Some(0x2764));
            // The casing is Windows', not ours.
            assert_eq!(instance_pid(r"usb\vid_0a5c&pid_2712\6&x"), Some(0x2712));
            // Nothing to parse - fall back rather than bind a bogus PID.
            assert_eq!(instance_pid(r"USB\VID_0A5C\6&1D0E4B2"), None);
            assert_eq!(instance_pid(r"USB\VID_0A5C&PID_ZZZZ\6"), None);
        }
    }
}

// ---------------------------------------------------------------------------
// Everywhere else - nothing to do, but the commands must still exist so the
// frontend can call them unconditionally.
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "windows"))]
mod platform {
    use tauri::AppHandle;

    use super::{DriverStatus, RPIBOOT_INSTALLER_URL};

    pub fn status(_app: &AppHandle) -> DriverStatus {
        DriverStatus {
            applicable: false,
            device_present: false,
            driver_ok: true,
            can_install: false,
            installer_url: RPIBOOT_INSTALLER_URL,
            detail: "no driver needed on this platform".to_string(),
        }
    }

    pub fn install(_app: &AppHandle) -> Result<(), String> {
        Err("the WinUSB driver is only needed on Windows".to_string())
    }

    pub fn device_present() -> bool {
        false
    }
}
