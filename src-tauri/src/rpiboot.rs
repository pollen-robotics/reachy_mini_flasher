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
//! Resolved from (in order): env overrides, bundled resources, then $PATH.

use std::path::{Path, PathBuf};
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
    let bin = rpiboot_bin(app).ok_or_else(|| {
        "rpiboot was not found. Install it (see README) or set REACHY_RPIBOOT_BIN.".to_string()
    })?;
    let dir = gadget_dir(app).ok_or_else(|| {
        "rpiboot boot files (mass-storage-gadget64) were not found. See README or set \
         REACHY_RPIBOOT_DIR."
            .to_string()
    })?;
    run_elevated_rpiboot(&bin, &dir)
}

#[cfg(target_os = "macos")]
fn run_elevated_rpiboot(bin: &Path, dir: &Path) -> Result<(), String> {
    // osascript runs the command as root, so no `sudo` needed inside.
    let apple = format!(
        "do shell script \"'{}' -d '{}'\" with prompt \"Reachy Mini Flasher needs administrator access to prepare your robot for flashing.\" with administrator privileges",
        bin.to_string_lossy(),
        dir.to_string_lossy()
    );
    let out = Command::new("osascript")
        .args(["-e", &apple])
        .output()
        .map_err(|e| format!("failed to request privileges: {e}"))?;

    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("-128") {
            Err("Preparation cancelled (admin authorization denied).".to_string())
        } else {
            Err(format!("rpiboot failed: {}", err.trim()))
        }
    }
}

#[cfg(target_os = "windows")]
fn run_elevated_rpiboot(bin: &Path, dir: &Path) -> Result<(), String> {
    let ps = format!(
        "$p = Start-Process -FilePath '{}' -ArgumentList '-d','{}' -Verb RunAs -Wait -PassThru; exit $p.ExitCode",
        bin.to_string_lossy(),
        dir.to_string_lossy()
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .output()
        .map_err(|e| format!("failed to request privileges: {e}"))?;

    if out.status.success() {
        Ok(())
    } else {
        Err("rpiboot failed (is the WinUSB driver installed?)".to_string())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn run_elevated_rpiboot(_bin: &Path, _dir: &Path) -> Result<(), String> {
    Err("automatic rpiboot is only implemented for macOS and Windows".to_string())
}
