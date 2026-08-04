//! Drive `rpiboot` (raspberrypi/usbboot) to expose the CM4 eMMC.
//!
//! When a Reachy is plugged in with the switch on DOWNLOAD, its CM4 shows up
//! as a Broadcom USB device (vendor 0x0a5c) but the internal eMMC is NOT yet
//! visible as a disk. `rpiboot -d mass-storage-gadget64` uploads the
//! mass-storage bootcode so the eMMC appears as a block device we can flash.
//!
//! This mirrors the manual procedure documented in
//! `reachy_mini/docs/.../reflash_the_rpi_ISO.md` (`sudo ./rpiboot -d
//! mass-storage-gadget64`), but runs it automatically and elevated.
//!
//! Requirements (provided out-of-band, see README / scripts/fetch-rpiboot.sh):
//!   - the `rpiboot` binary,
//!   - the `mass-storage-gadget64` boot-files directory.
//!
//! Resolved from (in order): env overrides, bundled resources, then $PATH.

use std::path::{Path, PathBuf};
// Only the macOS path shells out directly; Windows elevation goes through
// `win_ps::run_elevated`.
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager};

use crate::sim;

/// Guards against launching several rpiboot runs at once (the frontend polls
/// detection every ~1.5s, so download mode would otherwise fire repeatedly).
static RUNNING: AtomicBool = AtomicBool::new(false);

fn bin_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "rpiboot.exe"
    } else {
        "rpiboot"
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|cand| cand.is_file())
}

/// Locate the rpiboot executable: env override -> bundled resource -> $PATH.
fn rpiboot_bin(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("REACHY_RPIBOOT_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(res) = app.path().resource_dir() {
        let cand = res.join("rpiboot").join(bin_name());
        if cand.exists() {
            return Some(cand);
        }
    }
    // Windows: reuse an existing RPiBoot installation. It's what we point users
    // at for the WinUSB driver, and it brings rpiboot.exe along with it.
    #[cfg(target_os = "windows")]
    if let Some(dir) = crate::win_driver::rpiboot_install_dir() {
        let cand = dir.join(bin_name());
        if cand.is_file() {
            return Some(cand);
        }
    }
    find_on_path(bin_name())
}

/// Locate the `mass-storage-gadget64` boot-files directory.
fn gadget_dir(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("REACHY_RPIBOOT_DIR") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(res) = app.path().resource_dir() {
        let cand = res.join("rpiboot").join("mass-storage-gadget64");
        if cand.exists() {
            return Some(cand);
        }
    }
    #[cfg(target_os = "windows")]
    if let Some(dir) = crate::win_driver::rpiboot_install_dir() {
        let cand = dir.join("mass-storage-gadget64");
        if cand.is_dir() {
            return Some(cand);
        }
    }
    None
}

/// Expose the CM4 eMMC via rpiboot, if needed.
///
/// No-op in simulation mode or when the eMMC is already exposed (`ready`).
/// Otherwise runs `rpiboot -d <gadget_dir>` with elevated privileges and
/// blocks until it completes; the eMMC then enumerates as a disk and the next
/// `detect_reachy` poll reports `ready`.
#[tauri::command]
pub async fn prepare_reachy(app: AppHandle) -> Result<(), String> {
    if sim::enabled() {
        return Ok(());
    }

    // Already exposed? nothing to do.
    if let Some(dev) = crate::detect::detect_reachy() {
        if dev.mode == "ready" {
            return Ok(());
        }
    }

    if RUNNING.swap(true, Ordering::SeqCst) {
        // A run is already in flight; let it finish.
        return Ok(());
    }

    let result = tauri::async_runtime::spawn_blocking(move || run_rpiboot(&app))
        .await
        .map_err(|e| format!("rpiboot task panicked: {e}"));

    RUNNING.store(false, Ordering::SeqCst);
    result?
}

fn run_rpiboot(app: &AppHandle) -> Result<(), String> {
    // Windows: with no WinUSB driver bound to the CM4, rpiboot can't open the
    // device at all. Say so up front - the UI turns this into an "install the
    // driver" action - instead of letting rpiboot fail with a generic error.
    #[cfg(target_os = "windows")]
    if let Some(reason) = crate::win_driver::blocking_reason(app) {
        return Err(reason);
    }

    let bin = rpiboot_bin(app).ok_or_else(|| {
        "rpiboot was not found. Install it (see README) or set REACHY_RPIBOOT_BIN.".to_string()
    })?;
    let dir = gadget_dir(app).ok_or_else(|| {
        "rpiboot boot files (mass-storage-gadget64) were not found. See README or set \
         REACHY_RPIBOOT_DIR."
            .to_string()
    })?;

    // On macOS the elevated (root-via-osascript) process is a separate TCC
    // subject that CANNOT read files under ~/Documents, ~/Desktop or ~/Downloads
    // - so if the artifacts live in such a folder (dev checkout), rpiboot sees an
    // empty dir ("No 'bootcode' files found") and bails. Copy them into a
    // non-protected cache dir first, in the app's own (user) context which *can*
    // read those folders.
    #[cfg(target_os = "macos")]
    let (bin, dir) = stage_artifacts_macos(app, &bin, &dir)?;

    run_elevated_rpiboot(&bin, &dir)
}

/// Copy the rpiboot binary and gadget dir into the app cache (outside the
/// TCC-protected folders) so the elevated process can read them.
#[cfg(target_os = "macos")]
fn stage_artifacts_macos(
    app: &AppHandle,
    bin: &Path,
    dir: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let base = app
        .path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("rpiboot-stage");

    // Start clean to avoid stale boot files across upgrades.
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).map_err(|e| format!("failed to create staging dir: {e}"))?;

    let staged_bin = base.join(bin_name());
    fs::copy(bin, &staged_bin).map_err(|e| format!("failed to stage rpiboot binary: {e}"))?;
    let mut perms = fs::metadata(&staged_bin)
        .map_err(|e| format!("failed to read staged binary metadata: {e}"))?
        .permissions();
    perms.set_mode(0o755);
    let _ = fs::set_permissions(&staged_bin, perms);

    let dir_name = dir
        .file_name()
        .ok_or_else(|| "invalid gadget directory path".to_string())?;
    let staged_dir = base.join(dir_name);
    // `cp -RL` dereferences symlinks so the staged copy is self-contained. This
    // runs as the app user, which can read the source even under ~/Documents.
    let status = Command::new("cp")
        .arg("-RL")
        .arg(dir)
        .arg(&base)
        .status()
        .map_err(|e| format!("failed to stage gadget files: {e}"))?;
    if !status.success() {
        return Err("failed to copy rpiboot boot files into the cache dir".to_string());
    }

    Ok((staged_bin, staged_dir))
}

#[cfg(target_os = "macos")]
fn run_elevated_rpiboot(bin: &Path, dir: &Path) -> Result<(), String> {
    // osascript runs the command as root, so no `sudo` needed inside.
    // `2>&1` is essential: `do shell script` only captures the inner command's
    // stdout, so without redirecting, rpiboot's real error (on stderr) is lost
    // and every failure looks like a vague generic error.
    let apple = format!(
        "do shell script \"'{}' -d '{}' 2>&1\" with prompt \"Reachy Mini Flasher needs administrator access to prepare your robot for flashing.\" with administrator privileges",
        bin.to_string_lossy(),
        dir.to_string_lossy()
    );
    // Run from a non-TCC-protected directory. The elevated child shell inherits
    // this cwd; if it's under ~/Documents, ~/Desktop or ~/Downloads (as in a dev
    // checkout), the root shell's startup `getcwd()` is denied by TCC and the
    // whole script dies with exit 255 before rpiboot ever runs.
    let out = Command::new("osascript")
        .current_dir("/")
        .args(["-e", &apple])
        .output()
        .map_err(|e| format!("failed to request privileges: {e}"))?;

    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        // Log the full output so the real cause is visible in the dev console.
        eprintln!("rpiboot failed:\n  stdout: {}\n  stderr: {}", stdout.trim(), stderr.trim());
        if stderr.contains("-128") {
            Err("Preparation cancelled (admin authorization denied).".to_string())
        } else {
            let detail = [stderr.trim(), stdout.trim()]
                .into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or("no output");
            Err(format!("rpiboot failed: {detail}"))
        }
    }
}

#[cfg(target_os = "windows")]
fn run_elevated_rpiboot(bin: &Path, dir: &Path) -> Result<(), String> {
    // The gadget directory is quoted: it sits under Program Files or the app's
    // resource dir, both of which contain spaces.
    let args = format!(r#"-d "{}""#, dir.to_string_lossy());
    match crate::win_ps::run_elevated(bin, &args, None)? {
        0 => Ok(()),
        code => Err(format!(
            "rpiboot failed (exit code {code}). Unplug and re-plug the USB cable with the \
             switch on DOWNLOAD, then try again."
        )),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn run_elevated_rpiboot(_bin: &Path, _dir: &Path) -> Result<(), String> {
    Err("automatic rpiboot is only implemented for macOS and Windows".to_string())
}
