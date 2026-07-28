mod app_update;
mod detect;
mod disks;
mod flash;
mod images;
mod rpiboot;
mod sim;

use app_update::AppUpdateStore;

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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppUpdateStore::new())
        .invoke_handler(tauri::generate_handler![
            detect::detect_reachy,
            rpiboot::prepare_reachy,
            images::prefetch_image,
            flash::flash_reachy,
            open_url,
            app_update::get_app_update_info,
            app_update::install_app_update
        ])
        .setup(|app| {
            // Self-update check, release builds only (a debug build has no
            // signed bundle to update to, and the endpoint 404s before the
            // first release). Fail-open: handled inside `start_update_check`.
            #[cfg(not(debug_assertions))]
            app_update::start_update_check(app.handle().clone());
            let _ = app;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Reachy Mini Flasher");
}
