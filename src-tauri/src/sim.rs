//! Simulation mode.
//!
//! When `REACHY_FLASHER_SIM` is set, the app fakes a connected Reachy, a small
//! local image and flashes to a temp file instead of a real disk. This lets the
//! whole UX (detect -> flash -> progress -> done) be exercised on any machine,
//! with no robot, no network and no admin privileges.

use std::path::PathBuf;

pub fn enabled() -> bool {
    std::env::var("REACHY_FLASHER_SIM").is_ok()
}

/// Directory holding the simulated image and target "disk".
pub fn dir() -> PathBuf {
    std::env::temp_dir().join("reachy-mini-flasher-sim")
}

/// Fake device path we "flash" to in simulation.
pub fn target_path() -> PathBuf {
    dir().join("sim-disk.img")
}
