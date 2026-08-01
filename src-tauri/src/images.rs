//! Locate the ReachyMiniOS image to flash.
//!
//! Resolution order (real mode):
//!   1. newest `*.img` / `*.img.gz` already in the local cache dir
//!   2. otherwise, download the latest release asset from
//!      `pollen-robotics/reachy-mini-os` (with progress)
//!
//! GitHub release assets are served from Azure Blob storage, which honours HTTP
//! `Range` requests. A single stream is throttled per-connection (~3 MB/s in
//! practice), so the ~1.7 GB image is downloaded with several parallel ranged
//! connections, saturating the link (~2x faster, on par with a CDN mirror).
//!
//! In simulation mode a small local image is generated on the fly.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use tauri::AppHandle;

use crate::flash::emit_download;
use crate::sim;

const RELEASES_API: &str =
    "https://api.github.com/repos/pollen-robotics/reachy-mini-os/releases/latest";

/// Parallel-download tuning. GitHub/Azure throttles each connection, so we open
/// several at once. 16 MiB chunks pulled from a shared work queue keep the
/// workers balanced (a slow chunk doesn't strand an idle worker).
const DL_WORKERS: usize = 8;
const DL_CHUNK: u64 = 16 * 1024 * 1024;
const DL_CHUNK_RETRIES: usize = 3;

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

/// What the latest GitHub release ships: version tag + the image asset (and an
/// optional `.bmap`). Used both to decide whether the cache is fresh and to
/// download when it isn't.
struct ReleaseInfo {
    version: String,
    img_name: String,
    img_url: String,
    bmap: Option<(String, String)>,
}

/// Find the ISO, downloading it if needed. Emits `downloading` progress.
///
/// The cache is version-aware: we ask GitHub what the latest release ships and
/// only reuse the cache when it already holds *that* exact asset. An older image
/// left over from a previous run is refreshed. If GitHub is unreachable we fall
/// back to whatever image is cached so an offline flash still works.
pub fn resolve_image(app: &AppHandle) -> Result<ResolvedImage, String> {
    if sim::enabled() {
        emit_download(app, SIM_OS_VERSION, 0, 0);
        return resolve_sim_image();
    }

    let dir = cache_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create cache dir: {e}"))?;

    let client = reqwest::blocking::Client::builder()
        .user_agent("reachy-mini-flasher")
        .build()
        .map_err(|e| e.to_string())?;

    match fetch_latest_release(&client) {
        Ok(rel) => {
            let image_path = dir.join(&rel.img_name);
            if image_path.exists() {
                // Cache already holds exactly the latest asset - no download.
                emit_download(app, &rel.version, 0, 0);
                return Ok(ResolvedImage {
                    image_path: image_path.to_string_lossy().to_string(),
                    bmap_path: find_bmap(&dir),
                });
            }
            // Empty cache or a stale (older) version: fetch the new image, then
            // drop the old ones so we don't keep several ~1.7 GB files around.
            emit_download(app, &rel.version, 0, 0);
            let resolved = download_release(app, &client, &dir, &rel)?;
            let keep: Vec<&str> = std::iter::once(rel.img_name.as_str())
                .chain(rel.bmap.as_ref().map(|(n, _)| n.as_str()))
                .collect();
            remove_stale_files(&dir, &keep);
            Ok(resolved)
        }
        Err(net_err) => {
            // Offline / GitHub unreachable: reuse a cached image if we have one.
            if let Some(found) = newest_local_image(&dir)? {
                emit_download(app, "latest", 0, 0);
                return Ok(found);
            }
            Err(format!("could not fetch latest release (and no local image found): {net_err}"))
        }
    }
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

/// Query the GitHub releases API and pick the image (+ bmap) asset from the
/// latest release. Network/parse failures bubble up so the caller can fall back
/// to the cache.
fn fetch_latest_release(client: &reqwest::blocking::Client) -> Result<ReleaseInfo, String> {
    let body = client
        .get(RELEASES_API)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .text()
        .map_err(|e| e.to_string())?;

    let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let version = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("latest")
        .to_string();
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

    Ok(ReleaseInfo {
        version,
        img_name,
        img_url,
        bmap: find(&|n| n.to_lowercase().ends_with(".bmap")),
    })
}

/// Download the image (and bmap) described by `rel` into `dir`.
fn download_release(
    app: &AppHandle,
    client: &reqwest::blocking::Client,
    dir: &Path,
    rel: &ReleaseInfo,
) -> Result<ResolvedImage, String> {
    let image_path = dir.join(&rel.img_name);
    download_parallel(app, client, &rel.img_url, &image_path, &rel.version)?;

    let bmap_path = if let Some((bmap_name, bmap_url)) = &rel.bmap {
        let p = dir.join(bmap_name);
        download_file(app, client, bmap_url, &p, &rel.version, false)?;
        Some(p.to_string_lossy().to_string())
    } else {
        None
    };

    Ok(ResolvedImage {
        image_path: image_path.to_string_lossy().to_string(),
        bmap_path,
    })
}

/// Delete cached image/bmap/part files not in `keep` (e.g. an older OS version
/// left over after refreshing to a newer release), so the cache holds exactly
/// the current release's assets.
fn remove_stale_files(dir: &Path, keep: &[&str]) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if keep.contains(&name) {
            continue;
        }
        let is_bmap = name.to_lowercase().ends_with(".bmap");
        if is_image_name(name) || is_bmap || name.ends_with(".part") {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Download `url` to `dest` over several parallel ranged connections.
///
/// A single GitHub/Azure connection is throttled (~3 MB/s), leaving the link
/// mostly idle. We probe the total size (and confirm Range support) with a
/// 1-byte ranged GET, preallocate the `.part`, then let `DL_WORKERS` workers
/// pull `DL_CHUNK`-sized chunks from a shared cursor and write each to its byte
/// offset. Falls back to a single stream if the server ignores `Range`.
fn download_parallel(
    app: &AppHandle,
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    version: &str,
) -> Result<(), String> {
    let Some(total) = probe_total_with_ranges(client, url) else {
        // Ranges unsupported or size unknown: a single stream is the best we can do.
        return download_file(app, client, url, dest, version, true);
    };
    let report = |w: u64, t: u64| emit_download(app, version, w, t);
    download_ranged(client, url, dest, total, &report)
}

/// Core of the parallel downloader, decoupled from Tauri so it can be tested.
/// Preallocates `dest.part`, has `DL_WORKERS` workers pull `DL_CHUNK` ranges
/// from a shared cursor, writes each to its offset, then atomically renames.
/// `report(downloaded, total)` is invoked periodically from a monitor thread.
fn download_ranged(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    total: u64,
    report: &(dyn Fn(u64, u64) + Sync),
) -> Result<(), String> {
    let part = dest.with_extension("part");
    let _ = fs::remove_file(&part);
    {
        let f = fs::File::create(&part).map_err(|e| e.to_string())?;
        f.set_len(total)
            .map_err(|e| format!("failed to preallocate download: {e}"))?;
    }

    let num_chunks = total.div_ceil(DL_CHUNK);
    let next = AtomicUsize::new(0);
    let downloaded = AtomicU64::new(0);
    let failed = AtomicBool::new(false);
    let first_err = std::sync::Mutex::new(None::<String>);

    report(0, total);

    std::thread::scope(|scope| {
        // Progress monitor: emit the aggregated byte count until workers finish.
        scope.spawn(|| {
            loop {
                let done = downloaded.load(Ordering::Relaxed);
                report(done, total);
                if done >= total || failed.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });

        for _ in 0..DL_WORKERS {
            scope.spawn(|| loop {
                if failed.load(Ordering::Relaxed) {
                    return;
                }
                let idx = next.fetch_add(1, Ordering::Relaxed) as u64;
                if idx >= num_chunks {
                    return;
                }
                let start = idx * DL_CHUNK;
                let end = (start + DL_CHUNK).min(total) - 1;
                if let Err(e) = download_chunk(client, url, &part, start, end, &downloaded) {
                    let mut slot = first_err.lock().unwrap();
                    if slot.is_none() {
                        *slot = Some(e);
                    }
                    failed.store(true, Ordering::Relaxed);
                    return;
                }
            });
        }
    });

    if let Some(e) = first_err.into_inner().unwrap() {
        let _ = fs::remove_file(&part);
        return Err(format!("download failed: {e}"));
    }

    // Sum of chunk lengths equals `total` exactly, so this also catches a
    // truncated transfer being finalized as a complete image.
    let got = downloaded.load(Ordering::Relaxed);
    if got != total {
        let _ = fs::remove_file(&part);
        return Err(format!(
            "download incomplete: expected {total} bytes, got {got}. Please retry."
        ));
    }

    fs::rename(&part, dest).map_err(|e| {
        let _ = fs::remove_file(&part);
        format!("failed to finalize download: {e}")
    })?;
    report(total, total);
    Ok(())
}

/// Probe the asset size and confirm the server honours `Range`. Returns the
/// total length parsed from the `Content-Range` of a 1-byte ranged GET, or
/// `None` if the server replied with a full `200` (Range ignored).
fn probe_total_with_ranges(client: &reqwest::blocking::Client, url: &str) -> Option<u64> {
    let resp = client
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return None;
    }
    // Content-Range looks like "bytes 0-0/1695617571".
    let cr = resp.headers().get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    let total = cr.rsplit('/').next()?.trim().parse::<u64>().ok()?;
    (total > 0).then_some(total)
}

/// Download one byte range into `part` at its offset, retrying transient
/// failures. On success, advances the shared `downloaded` counter by the chunk
/// length (only once, so retries never inflate the total).
fn download_chunk(
    client: &reqwest::blocking::Client,
    url: &str,
    part: &Path,
    start: u64,
    end: u64,
    downloaded: &AtomicU64,
) -> Result<(), String> {
    let len = end - start + 1;
    let mut attempt = 0;
    loop {
        attempt += 1;
        match try_download_chunk(client, url, part, start, end) {
            Ok(()) => {
                downloaded.fetch_add(len, Ordering::Relaxed);
                return Ok(());
            }
            Err(e) => {
                if attempt >= DL_CHUNK_RETRIES {
                    return Err(e);
                }
                std::thread::sleep(std::time::Duration::from_millis(300 * attempt as u64));
            }
        }
    }
}

fn try_download_chunk(
    client: &reqwest::blocking::Client,
    url: &str,
    part: &Path,
    start: u64,
    end: u64,
) -> Result<(), String> {
    let mut resp = client
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    // A 200 means the server ignored Range and is about to stream the whole
    // file into this one chunk's offset - refuse rather than corrupt the image.
    if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(format!("expected 206 for range {start}-{end}, got {}", resp.status()));
    }

    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(part)
        .map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;

    let mut buf = vec![0u8; 256 * 1024];
    let mut written = 0u64;
    loop {
        let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        written += n as u64;
    }
    file.flush().map_err(|e| e.to_string())?;

    let expected = end - start + 1;
    if written != expected {
        return Err(format!(
            "range {start}-{end} truncated: expected {expected} bytes, got {written}"
        ));
    }
    Ok(())
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

    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    /// Minimal HTTP/1.1 server that honours `Range: bytes=a-b` (206 + a slice)
    /// and serves the whole blob otherwise (200). One thread per connection so
    /// parallel workers are served concurrently, mirroring GitHub/Azure.
    fn spawn_range_server(data: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let data = std::sync::Arc::new(data);
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let data = data.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut range: Option<(u64, u64)> = None;
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        let l = line.trim_end();
                        if l.is_empty() {
                            break;
                        }
                        let lower = l.to_ascii_lowercase();
                        if let Some(v) = lower.strip_prefix("range: bytes=") {
                            let mut it = v.split('-');
                            let a = it.next().and_then(|s| s.trim().parse::<u64>().ok());
                            let b = it.next().and_then(|s| s.trim().parse::<u64>().ok());
                            if let (Some(a), Some(b)) = (a, b) {
                                range = Some((a, b));
                            }
                        }
                    }
                    let total = data.len() as u64;
                    match range {
                        Some((a, b)) => {
                            let b = b.min(total - 1);
                            let slice = &data[a as usize..=b as usize];
                            let hdr = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {a}-{b}/{total}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                                slice.len()
                            );
                            let _ = stream.write_all(hdr.as_bytes());
                            let _ = stream.write_all(slice);
                        }
                        None => {
                            let hdr = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
                            );
                            let _ = stream.write_all(hdr.as_bytes());
                            let _ = stream.write_all(&data);
                        }
                    }
                    let _ = stream.flush();
                });
            }
        });
        format!("http://{addr}/img")
    }

    fn patterned_blob(len: usize) -> Vec<u8> {
        // Position-dependent bytes so a mis-offset chunk is detected.
        let mut data = vec![0u8; len];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = (i as u64).wrapping_mul(2654435761).wrapping_shr(11) as u8;
        }
        data
    }

    /// Real-network check: GitHub release download URLs 302-redirect to Azure
    /// Blob, and the parallel downloader only works if `reqwest` carries the
    /// `Range` header across that redirect (else we'd get a full 200 per chunk).
    /// Ignored by default so the suite stays offline; run with `--ignored`.
    #[test]
    #[ignore]
    fn real_github_asset_supports_ranges_through_redirect() {
        let url = "https://github.com/pollen-robotics/reachy-mini-os/releases/download/v0.2.7/2026-06-17-reachyminios-v0.2.7.bmap";
        let client = reqwest::blocking::Client::builder()
            .user_agent("reachy-mini-flasher")
            .build()
            .unwrap();
        let total = probe_total_with_ranges(&client, url)
            .expect("probe should see 206 + Content-Range through the redirect");
        assert!(total > 0);

        let dir = std::env::temp_dir().join(format!("rmf-realdl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("real.bmap");
        let report = |_w: u64, _t: u64| {};
        download_ranged(&client, url, &dest, total, &report).expect("real download ok");
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), total);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_reports_total_and_range_support() {
        let data = patterned_blob(3 * 1024 * 1024);
        let expected = data.len() as u64;
        let url = spawn_range_server(data);
        let client = reqwest::blocking::Client::new();
        assert_eq!(probe_total_with_ranges(&client, &url), Some(expected));
    }

    #[test]
    fn parallel_download_reassembles_bytes_exactly() {
        // ~40 MiB + odd tail => spans several DL_CHUNKs with a partial last one,
        // exercising multi-worker assembly and offset correctness.
        let expected = patterned_blob(40 * 1024 * 1024 + 12_345);
        let total = expected.len() as u64;
        let url = spawn_range_server(expected.clone());
        let client = reqwest::blocking::Client::new();

        let dir = std::env::temp_dir().join(format!("rmf-dltest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("out.bin");

        let last_progress = AtomicU64::new(0);
        let report = |w: u64, _t: u64| last_progress.store(w, Ordering::Relaxed);
        download_ranged(&client, &url, &dest, total, &report).expect("download should succeed");

        let got = std::fs::read(&dest).unwrap();
        assert_eq!(got.len(), expected.len(), "downloaded size mismatch");
        assert!(got == expected, "downloaded bytes differ from source");
        assert_eq!(last_progress.load(Ordering::Relaxed), total, "final progress != total");
        // The intermediate `.part` must be gone after the atomic rename.
        assert!(!dest.with_extension("part").exists(), "leftover .part file");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
