//! Locate the ReachyMiniOS image to flash.
//!
//! Resolution order (real mode):
//!   1. newest `*.img` / `*.img.gz` already in the local cache dir
//!   2. otherwise, download the latest release asset from
//!      `pollen-robotics/reachy-mini-os` (with progress)
//!
//! In simulation mode a small local image is generated on the fly.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tauri::AppHandle;

use crate::flash::emit_download;
use crate::sim;

const RELEASES_API: &str =
    "https://api.github.com/repos/pollen-robotics/reachy-mini-os/releases/latest";

/// Fast mirror: a public HF Storage Bucket (xet CDN) is much quicker and more
/// reliable than GitHub releases. Public bucket files are served anonymously at
/// `.../buckets/<ns>/<name>/resolve/<path>` (buckets are non-versioned, so there
/// is no revision segment). `latest.json` (published by the mirror CI) points at
/// the current image/bmap; both are fetched from the same base. GitHub is the
/// fallback.
const HF_RESOLVE_BASE: &str =
    "https://huggingface.co/buckets/pollen-robotics/reachy-mini-os/resolve";

pub struct ResolvedImage {
    pub image_path: String,
    pub bmap_path: Option<String>,
}

/// Download the OS image into the local cache ahead of time (called at startup),
/// so the user never waits for it at flash time. Emits `downloading` progress.
#[tauri::command]
pub async fn prefetch_image(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || resolve_image(&app).map(|_| ()))
        .await
        .map_err(|e| format!("prefetch task panicked: {e}"))?
}

fn cache_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("reachy-mini-flasher")
        .join("images")
}

/// Version reported in simulation mode (no real OS is written).
const SIM_OS_VERSION: &str = "1.8.4";

/// Find the ISO, downloading it if needed. Emits `downloading` progress.
pub fn resolve_image(app: &AppHandle) -> Result<ResolvedImage, String> {
    if sim::enabled() {
        emit_download(app, SIM_OS_VERSION, 0, 0);
        return resolve_sim_image();
    }

    let dir = cache_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create cache dir: {e}"))?;

    if let Some(found) = newest_local_image(&dir)? {
        // The image is already cached, so no download happens - but the UI still
        // wants the version. Surface it from the mirror manifest (best-effort).
        emit_download(app, &cached_os_version(), 0, 0);
        return Ok(found);
    }

    // Prefer the fast HF mirror; fall back to GitHub releases if unavailable.
    match download_from_hf(app, &dir) {
        Ok(found) => Ok(found),
        Err(hf_err) => download_latest_release(app, &dir).map_err(|gh_err| {
            format!("HF mirror unavailable ({hf_err}); GitHub fallback failed: {gh_err}")
        }),
    }
}

/// Download the image (and bmap) from the Hugging Face bucket mirror, using the
/// `latest.json` manifest to locate the current release assets.
fn download_from_hf(app: &AppHandle, dir: &Path) -> Result<ResolvedImage, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("reachy-mini-flasher")
        .build()
        .map_err(|e| e.to_string())?;

    let manifest_url = format!("{HF_RESOLVE_BASE}/latest.json");
    let body = client
        .get(&manifest_url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("manifest fetch failed: {e}"))?
        .text()
        .map_err(|e| e.to_string())?;

    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("manifest parse failed: {e}"))?;
    let tag = json.get("tag").and_then(|v| v.as_str()).unwrap_or("latest").to_string();
    let image_rel = json
        .get("image")
        .and_then(|v| v.as_str())
        .ok_or("manifest has no image entry")?;

    emit_download(app, &tag, 0, 0);

    let basename = |rel: &str| rel.rsplit('/').next().unwrap_or(rel).to_string();

    let image_name = basename(image_rel);
    let image_path = dir.join(&image_name);
    download_file(
        app,
        &client,
        &format!("{HF_RESOLVE_BASE}/{image_rel}"),
        &image_path,
        &tag,
        true,
    )?;

    // Verify the download against the manifest checksum before we ever touch the
    // eMMC. This catches a corrupt/partial upload (or CDN mishap) early, and the
    // "checksum" wording makes `looks_like_corrupt_image` trigger a re-download.
    if let Some(expected) = json.get("sha256").and_then(|v| v.as_str()) {
        let actual = sha256_file(&image_path)?;
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            let _ = fs::remove_file(&image_path);
            return Err(format!(
                "image checksum mismatch: expected {expected}, got {actual}. Please retry."
            ));
        }
    }

    let bmap_path = if let Some(bmap_rel) = json.get("bmap").and_then(|v| v.as_str()) {
        let p = dir.join(basename(bmap_rel));
        download_file(app, &client, &format!("{HF_RESOLVE_BASE}/{bmap_rel}"), &p, &tag, false)?;
        Some(p.to_string_lossy().to_string())
    } else {
        None
    };

    Ok(ResolvedImage {
        image_path: image_path.to_string_lossy().to_string(),
        bmap_path,
    })
}

/// Best-effort OS version when flashing from cache: the mirror manifest `tag`,
/// falling back to "latest" when offline.
fn cached_os_version() -> String {
    fetch_manifest_tag().unwrap_or_else(|| "latest".to_string())
}

/// Quick fetch of the current release tag from the HF mirror manifest.
fn fetch_manifest_tag() -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("reachy-mini-flasher")
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    let body = client
        .get(format!("{HF_RESOLVE_BASE}/latest.json"))
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    json.get("tag").and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn is_image_name(name: &str) -> bool {
    let n = name.to_lowercase();
    n.ends_with(".img") || n.ends_with(".img.gz") || n.ends_with(".zip")
}

/// Remove cached image files (and any partial downloads) so the next resolve
/// re-downloads a fresh copy. Called when a flash fails on a corrupt image.
/// The small `.bmap` is left in place; it is re-fetched only if missing.
pub fn purge_cache() {
    let dir = cache_dir();
    let Ok(entries) = fs::read_dir(&dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        if is_image_name(&name) || name.ends_with(".part") {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Whether a flash error looks like a corrupt/truncated image (as opposed to a
/// hardware/permission problem), meaning re-downloading is worth trying.
pub fn looks_like_corrupt_image(err: &str) -> bool {
    let e = err.to_lowercase();
    [
        "corrupt",
        "deflate",
        "inflate",
        "failed to read",
        "checksum",
        "unexpected eof",
        "eocd",
        "central directory",
        "invalid zip",
        "invalid gzip",
        "no .img entry",
    ]
    .iter()
    .any(|needle| e.contains(needle))
}

fn newest_local_image(dir: &Path) -> Result<Option<ResolvedImage>, String> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if is_image_name(name) {
            let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            if best.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
                best = Some((modified, path));
            }
        }
    }

    Ok(best.map(|(_, image)| ResolvedImage {
        image_path: image.to_string_lossy().to_string(),
        bmap_path: find_bmap(dir),
    }))
}

/// A release ships exactly one `.bmap`, so any `.bmap` in the cache dir is it.
fn find_bmap(dir: &Path) -> Option<String> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().map(|x| x.eq_ignore_ascii_case("bmap")).unwrap_or(false))
        .map(|p| p.to_string_lossy().to_string())
}

fn download_latest_release(app: &AppHandle, dir: &Path) -> Result<ResolvedImage, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("reachy-mini-flasher")
        .build()
        .map_err(|e| e.to_string())?;

    let body = client
        .get(RELEASES_API)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("could not fetch latest release (and no local image found): {e}"))?
        .text()
        .map_err(|e| e.to_string())?;

    let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let version = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("latest")
        .to_string();
    // Surface the version immediately, before the (large) download starts.
    emit_download(app, &version, 0, 0);
    let assets = json.get("assets").and_then(|a| a.as_array()).cloned().unwrap_or_default();

    let find = |pred: &dyn Fn(&str) -> bool| -> Option<(String, String)> {
        assets.iter().find_map(|a| {
            let name = a.get("name")?.as_str()?;
            let url = a.get("browser_download_url")?.as_str()?;
            pred(name).then(|| (name.to_string(), url.to_string()))
        })
    };

    let (img_name, img_url) = find(&|n| n.to_lowercase().ends_with(".img.gz"))
        .or_else(|| find(&|n| n.to_lowercase().ends_with(".zip")))
        .or_else(|| find(&|n| n.to_lowercase().ends_with(".img")))
        .ok_or_else(|| "no image asset (.img.gz/.zip/.img) found in the latest release".to_string())?;

    let image_path = dir.join(&img_name);
    download_file(app, &client, &img_url, &image_path, &version, true)?;

    let bmap_path = if let Some((bmap_name, bmap_url)) =
        find(&|n| n.to_lowercase().ends_with(".bmap"))
    {
        let p = dir.join(&bmap_name);
        download_file(app, &client, &bmap_url, &p, &version, false)?;
        Some(p.to_string_lossy().to_string())
    } else {
        None
    };

    Ok(ResolvedImage {
        image_path: image_path.to_string_lossy().to_string(),
        bmap_path,
    })
}

/// Stream a file through SHA-256 and return the lowercase hex digest. Used to
/// verify a downloaded image against the mirror manifest before flashing.
fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("failed to open for hashing: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("failed to read: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn download_file(
    app: &AppHandle,
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    version: &str,
    report: bool,
) -> Result<(), String> {
    // Always download fresh to a `.part`, then atomically rename. We do NOT
    // resume from an existing `.part`: a mismatched Range/redirect response can
    // splice a partial and a full body together, producing a same-or-larger but
    // corrupt file (bad central directory / deflate). Correctness > resume.
    let part = dest.with_extension("part");
    let _ = fs::remove_file(&part);

    let mut resp = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("download failed: {e}"))?;

    let total = resp.content_length().unwrap_or(0);
    let mut file = fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;
    let mut buf = vec![0u8; 1024 * 1024];
    let mut last_emit: u64 = 0;

    loop {
        let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        downloaded += n as u64;
        if report && downloaded - last_emit >= 2 * 1024 * 1024 {
            emit_download(app, version, downloaded, total);
            last_emit = downloaded;
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    // Guard against a truncated transfer being finalized as a complete image.
    if total > 0 && downloaded != total {
        let _ = fs::remove_file(&part);
        return Err(format!(
            "download incomplete: expected {total} bytes, got {downloaded}. Please retry."
        ));
    }

    fs::rename(&part, dest).map_err(|e| {
        let _ = fs::remove_file(&part);
        format!("failed to finalize download: {e}")
    })?;
    if report {
        emit_download(app, version, downloaded, total);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_image_is_generated_on_disk() {
        let resolved = resolve_sim_image().expect("sim image should resolve");
        let path = std::path::Path::new(&resolved.image_path);
        assert!(path.exists(), "sim image file must exist");
        let len = std::fs::metadata(path).unwrap().len();
        assert!(len >= 48 * 1024 * 1024, "sim image should be ~48 MB, got {len}");
        assert!(resolved.bmap_path.is_none(), "sim image has no bmap");
    }
}

fn resolve_sim_image() -> Result<ResolvedImage, String> {
    let dir = sim::dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let image = dir.join("sim-image.img");

    // Generate a ~48 MB image once, with a recognizable pattern.
    if !image.exists() {
        let mut file = fs::File::create(&image).map_err(|e| e.to_string())?;
        let chunk = vec![0xA5u8; 1024 * 1024];
        for _ in 0..48 {
            file.write_all(&chunk).map_err(|e| e.to_string())?;
        }
    }

    Ok(ResolvedImage {
        image_path: image.to_string_lossy().to_string(),
        bmap_path: None,
    })
}
