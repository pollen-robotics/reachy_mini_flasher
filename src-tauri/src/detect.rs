//! Detect a Reachy Mini connected over USB.
//!
//! Two real signals:
//!   - `ready`: the CM4 eMMC is exposed as a mass-storage disk (after rpiboot).
//!     Recognized by the disk description (RPi-MSD / Compute Module / File-Stor).
//!   - `download`: the CM4 is in bootloader/download mode (USB vendor 0x0a5c),
//!     eMMC not yet exposed. The user must wait / rpiboot must run.
//!
//! In simulation mode a fake `simulated` device is returned.

use std::sync::OnceLock;
use std::time::Instant;

use serde::Serialize;

use crate::sim;

/// Broadcom vendor id, used by the BCM2711 while in USB boot/download mode.
const BROADCOM_VID: u16 = 0x0a5c;

/// In simulation mode, pretend the robot is plugged in only after this delay,
/// so the "waiting -> detected" flow is actually exercised (configurable via
/// `REACHY_FLASHER_SIM_DELAY`, in seconds).
static SIM_START: OnceLock<Instant> = OnceLock::new();

fn sim_connected() -> bool {
    let delay: u64 = std::env::var("REACHY_FLASHER_SIM_DELAY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    SIM_START.get_or_init(Instant::now).elapsed().as_secs() >= delay
}

#[derive(Serialize, Clone)]
pub struct ReachyDevice {
    /// Raw device path to flash. Empty when not yet flashable (download mode).
    pub device: String,
    pub display_device: String,
    pub description: String,
    pub size: u64,
    /// "ready" | "download" | "simulated"
    pub mode: String,
}

/// A disk whose description matches a Raspberry Pi mass-storage gadget.
///
/// Depending on the gadget used, macOS reports different media names:
///   - legacy `File-Stor Gadget` -> "Linux File-Stor Gadget Media"
///   - `mass-storage-gadget64` (rpiboot) -> "mmcblk0 Media" (the CM4 eMMC)
fn looks_like_reachy_disk(description: &str) -> bool {
    let d = description.to_lowercase();
    [
        "rpi-msd",
        "compute module",
        "file-stor",
        "raspberry",
        "rpi msd",
        "mmcblk",
    ]
    .iter()
    .any(|needle| d.contains(needle))
}

fn in_download_mode() -> bool {
    match nusb::list_devices() {
        Ok(devices) => devices.into_iter().any(|d| d.vendor_id() == BROADCOM_VID),
        Err(_) => false,
    }
}

/// Returns the currently connected Reachy, if any.
#[tauri::command]
pub fn detect_reachy() -> Option<ReachyDevice> {
    if sim::enabled() {
        if !sim_connected() {
            return None;
        }
        return Some(ReachyDevice {
            device: sim::target_path().to_string_lossy().to_string(),
            display_device: "Simulated Reachy Mini".to_string(),
            description: "Simulated Reachy Mini (CM4)".to_string(),
            size: 64 * 1024 * 1024,
            mode: "simulated".to_string(),
        });
    }

    if let Ok(disks) = crate::disks::list() {
        if let Some(disk) = disks.into_iter().find(|d| looks_like_reachy_disk(&d.description)) {
            return Some(ReachyDevice {
                device: disk.device,
                display_device: disk.display_device,
                description: disk.description,
                size: disk.size,
                mode: "ready".to_string(),
            });
        }
    }

    if in_download_mode() {
        return Some(ReachyDevice {
            device: String::new(),
            display_device: "Reachy Mini (CM4)".to_string(),
            description: "Reachy Mini in download mode".to_string(),
            size: 0,
            mode: "download".to_string(),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_mode_disk_signatures_match() {
        assert!(looks_like_reachy_disk("RPi-MSD- 0001 Media"));
        assert!(looks_like_reachy_disk("Linux File-Stor Gadget Media"));
        assert!(looks_like_reachy_disk("Compute Module 4"));
        assert!(looks_like_reachy_disk("mmcblk0 Media"));
        assert!(!looks_like_reachy_disk("Samsung Flash Drive"));
        assert!(!looks_like_reachy_disk("APPLE SSD"));
    }

    #[test]
    fn sim_env_returns_simulated_device() {
        std::env::set_var("REACHY_FLASHER_SIM", "1");
        std::env::set_var("REACHY_FLASHER_SIM_DELAY", "0");
        let dev = detect_reachy().expect("sim should report a device once connected");
        assert_eq!(dev.mode, "simulated");
        std::env::remove_var("REACHY_FLASHER_SIM");
        std::env::remove_var("REACHY_FLASHER_SIM_DELAY");
    }
}
