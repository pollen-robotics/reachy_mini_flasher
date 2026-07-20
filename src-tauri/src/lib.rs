mod detect;
mod disks;
mod flash;
mod images;
mod rpiboot;
mod sim;

pub use flash::run_flash_worker;

/// Open an external URL in the user's default browser.
///
/// Restricted to http(s) so the command can't be abused to launch arbitrary
/// local programs. Uses the platform's native "open" helper - no extra Tauri
/// plugin needed.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("only http(s) URLs are allowed".into());
    }

    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(target_os = "windows")]
    let spawned = std::process::Command::new("cmd")
        .args(["/C", "start", "", &url])
        .spawn();
    #[cfg(target_os = "linux")]
    let spawned = std::process::Command::new("xdg-open").arg(&url).spawn();

    spawned.map(|_| ()).map_err(|e| format!("failed to open url: {e}"))
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            detect::detect_reachy,
            rpiboot::prepare_reachy,
            images::prefetch_image,
            flash::flash_reachy,
            open_url
        ])
        .run(tauri::generate_context!())
        .expect("error while running Reachy Mini Flasher");
}
