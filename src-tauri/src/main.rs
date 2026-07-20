// Prevent an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Elevated child process that performs the raw disk write.
    if args.get(1).map(|s| s == "flash-worker").unwrap_or(false) {
        reachy_mini_flasher_lib::run_flash_worker(&args[2..]);
        return;
    }

    reachy_mini_flasher_lib::run()
}
