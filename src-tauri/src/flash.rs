//! Flashing engine + orchestration.
//!
//! - `flash_reachy` (command): detect -> resolve image -> flash, streaming a
//!   single `flash://progress` event with a `phase` field.
//! - Real disk writes need root/admin, so they run in an elevated copy of this
//!   same binary (`flash-worker` subcommand). The GUI tails a progress file.
//! - In simulation mode the copy runs in-process to a temp file (no elevation).

use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::path::Path;

use bmap_parser::{Bmap, Discarder, SeekForward};
use flate2::read::GzDecoder;
use tauri::{AppHandle, Emitter};

use crate::sim;

pub const PROGRESS_EVENT: &str = "flash://progress";
const EMIT_EVERY_BYTES: u64 = 16 * 1024 * 1024;

pub fn emit_progress(app: &AppHandle, phase: &str, written: u64, total: u64) {
    let _ = app.emit(
        PROGRESS_EVENT,
        serde_json::json!({ "phase": phase, "written": written, "total": total }),
    );
}

/// Download progress carrying the OS version being fetched.
pub fn emit_download(app: &AppHandle, version: &str, written: u64, total: u64) {
    let _ = app.emit(
        PROGRESS_EVENT,
        serde_json::json!({
            "phase": "downloading",
            "written": written,
            "total": total,
            "version": version,
        }),
    );
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn flash_reachy(app: AppHandle) -> Result<(), String> {
    let device = crate::detect::detect_reachy().ok_or("No Reachy Mini detected over USB.")?;
    if device.mode == "download" {
        return Err(
            "Reachy is still in download mode; its storage isn't exposed yet. Wait for preparation to finish, or re-plug the USB cable."
                .to_string(),
        );
    }

    let sim = sim::enabled();
    let target = device.device.clone();

    tauri::async_runtime::spawn_blocking(move || run_flash(app, sim, target))
        .await
        .map_err(|e| format!("flash task panicked: {e}"))?
}

fn run_flash(app: AppHandle, is_sim: bool, target: String) -> Result<(), String> {
    // Simulation: don't touch a real disk (nor the tiny temp image, which would
    // flash in a blink). Emit synthetic progress over a few seconds so the whole
    // "writing image" UX - progress bar filling, then "done" - is exercised.
    if is_sim {
        return run_sim_flash(&app);
    }

    let image = crate::images::resolve_image(&app)?;

    // macOS: `authopen` (Apple, setuid) opens the device with admin auth and
    // hands us the fd. No osascript, no Full Disk Access, no worker process.
    #[cfg(target_os = "macos")]
    {
        let app_cb = app.clone();
        let report: Box<dyn FnMut(u64, u64) + Send> =
            Box::new(move |w, t| emit_progress(&app_cb, "flashing", w, t));
        match do_flash(&image.image_path, image.bmap_path.as_deref(), &target, report) {
            Ok(()) => {
                emit_progress(&app, "done", 1, 1);
                Ok(())
            }
            Err(e) if crate::images::looks_like_corrupt_image(&e) => {
                crate::images::purge_cache();
                Err(format!(
                    "{e}\n\nThe downloaded image was corrupt and has been removed. \
                     Relaunch to download a fresh copy, then flash again."
                ))
            }
            Err(e) => Err(e),
        }
    }

    // Windows: relaunch this binary elevated (RunAs) as a flash-worker.
    #[cfg(not(target_os = "macos"))]
    {
        flash_elevated(&app, &image.image_path, image.bmap_path.as_deref(), &target)
    }
}

/// Fake a flash so the "writing image" screen (progress bar + "done") is fully
/// exercised in simulation mode, with no disk and no elevation.
fn run_sim_flash(app: &AppHandle) -> Result<(), String> {
    use std::thread::sleep;
    use std::time::Duration;

    // Pretend we're writing a ~12 GB image so the numbers look realistic.
    let total: u64 = 12 * 1024 * 1024 * 1024;
    let steps: u64 = 60;

    emit_progress(app, "flashing", 0, total);
    for i in 1..=steps {
        sleep(Duration::from_millis(100));
        emit_progress(app, "flashing", total * i / steps, total);
    }
    emit_progress(app, "done", 1, 1);
    Ok(())
}

// ---------------------------------------------------------------------------
// Core copy (bmap-parser)
// ---------------------------------------------------------------------------

struct ProgressWriter<W> {
    inner: W,
    written: u64,
    total: u64,
    last_emit: u64,
    report: Box<dyn FnMut(u64, u64) + Send>,
}

impl<W: Write> Write for ProgressWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        if self.written - self.last_emit >= EMIT_EVERY_BYTES {
            (self.report)(self.written, self.total);
            self.last_emit = self.written;
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<W: SeekForward> SeekForward for ProgressWriter<W> {
    fn seek_forward(&mut self, offset: u64) -> std::io::Result<()> {
        self.inner.seek_forward(offset)
    }
}

/// Aligns all writes to a block boundary before hitting the device.
///
/// Raw disk devices (macOS `/dev/rdiskN`, Windows `\\.\PhysicalDriveN`) reject
/// writes whose length or offset is not a multiple of the sector size with
/// `EINVAL`. We buffer bytes and only ever `write` multiples of `ALIGN`; the
/// final short block is zero-padded up to `ALIGN` by `finish()`.
const ALIGN: usize = 4096;
const FLUSH_CHUNK: usize = 4 * 1024 * 1024;

struct SectorWriter<W: Write> {
    inner: W,
    buf: Vec<u8>,
}

impl<W: Write> SectorWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner, buf: Vec::with_capacity(FLUSH_CHUNK + ALIGN) }
    }

    /// Write the largest whole-block prefix of the buffer, keeping the tail.
    fn drain_aligned(&mut self) -> io::Result<()> {
        let n = (self.buf.len() / ALIGN) * ALIGN;
        if n > 0 {
            self.inner.write_all(&self.buf[..n])?;
            self.buf.drain(..n);
        }
        Ok(())
    }

    /// Flush the remaining bytes, zero-padded up to a block boundary. Call once
    /// at the very end (padding mid-stream would misalign later writes).
    fn finish(&mut self) -> io::Result<()> {
        self.drain_aligned()?;
        if !self.buf.is_empty() {
            let pad = (ALIGN - self.buf.len() % ALIGN) % ALIGN;
            self.buf.resize(self.buf.len() + pad, 0);
            self.inner.write_all(&self.buf)?;
            self.buf.clear();
        }
        self.inner.flush()
    }
}

impl<W: Write> Write for SectorWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= FLUSH_CHUNK {
            self.drain_aligned()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Only flush whole blocks; the partial tail waits for finish().
        self.drain_aligned()?;
        self.inner.flush()
    }
}

impl<W: Write + SeekForward> SeekForward for SectorWriter<W> {
    fn seek_forward(&mut self, offset: u64) -> io::Result<()> {
        // bmap regions are block-aligned, so the buffer is empty on a boundary
        // here; pad defensively just in case, then seek.
        self.drain_aligned()?;
        if !self.buf.is_empty() {
            let pad = (ALIGN - self.buf.len() % ALIGN) % ALIGN;
            self.buf.resize(self.buf.len() + pad, 0);
            self.inner.write_all(&self.buf)?;
            self.buf.clear();
        }
        self.inner.seek_forward(offset)
    }
}

/// Write `image_path` to `target`, using `bmap_path` when available.
pub fn do_flash(
    image_path: &str,
    bmap_path: Option<&str>,
    target: &str,
    mut report: Box<dyn FnMut(u64, u64) + Send>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if target.starts_with("/dev/") {
        let display = target.replacen("/dev/rdisk", "/dev/disk", 1);
        let _ = std::process::Command::new("diskutil")
            .args(["unmountDisk", "force", &display])
            .output();
    }

    let lower = image_path.to_lowercase();
    let is_gz = lower.ends_with(".gz");
    let is_zip = lower.ends_with(".zip");

    let bmap = match bmap_path {
        Some(p) => {
            let xml = fs::read_to_string(p).map_err(|e| format!("failed to read bmap: {e}"))?;
            Some(Bmap::from_xml(&xml).map_err(|e| format!("invalid bmap file: {e}"))?)
        }
        None => None,
    };

    let total = match &bmap {
        Some(b) => b.total_mapped_size(),
        None if is_gz || is_zip => 0,
        None => fs::metadata(image_path).map(|m| m.len()).unwrap_or(0),
    };

    report(0, total);

    let out = open_target(target)?;

    let image_file = fs::File::open(image_path).map_err(|e| format!("failed to open image: {e}"))?;

    if is_zip {
        // The release ships the .img inside a .zip; decompress the entry on the fly.
        let mut archive =
            zip::ZipArchive::new(image_file).map_err(|e| format!("invalid zip: {e}"))?;
        let mut img_name = None;
        for i in 0..archive.len() {
            let f = archive.by_index(i).map_err(|e| e.to_string())?;
            if f.name().to_lowercase().ends_with(".img") {
                img_name = Some(f.name().to_string());
                break;
            }
        }
        let img_name = img_name.ok_or_else(|| "no .img entry inside the zip".to_string())?;
        let entry = archive.by_name(&img_name).map_err(|e| e.to_string())?;
        let input = Discarder::new(entry);
        copy_input(input, out, bmap.as_ref(), total, report)
    } else if is_gz {
        let input = Discarder::new(GzDecoder::new(BufReader::new(image_file)));
        copy_input(input, out, bmap.as_ref(), total, report)
    } else {
        let input = BufReader::new(image_file);
        copy_input(input, out, bmap.as_ref(), total, report)
    }
}

/// Open the flash target for writing.
///
/// On macOS a `/dev/` device is opened via `authopen -stdoutpipe -w`, which
/// performs the privileged `open()` in an Apple-signed setuid binary and passes
/// the descriptor back over a socket (SCM_RIGHTS). This asks for admin
/// authorization but avoids the Full Disk Access requirement that hits any app
/// (even as root) opening a raw disk device directly.
fn open_target(target: &str) -> Result<fs::File, String> {
    #[cfg(target_os = "macos")]
    if target.starts_with("/dev/") {
        return open_via_authopen(target);
    }

    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(Path::new(target))
        .map_err(|e| format!("failed to open target '{target}': {e}"))
}

#[cfg(target_os = "macos")]
fn open_via_authopen(target: &str) -> Result<fs::File, String> {
    use std::os::fd::{FromRawFd, IntoRawFd};
    use std::os::unix::net::UnixStream;
    use std::process::{Command, Stdio};

    use sendfd::RecvWithFd;

    // authopen sends the opened fd over a UNIX-domain socket dup'd onto its
    // stdout, so we pair a socket and hand it one end.
    let (ours, theirs) =
        UnixStream::pair().map_err(|e| format!("failed to create socket pair: {e}"))?;
    let their_stdout = unsafe { Stdio::from_raw_fd(theirs.into_raw_fd()) };

    let mut child = Command::new("/usr/libexec/authopen")
        .args(["-stdoutpipe", "-w", target])
        .stdout(their_stdout)
        .spawn()
        .map_err(|e| format!("failed to launch authopen: {e}"))?;

    let mut buf = [0u8; 16];
    let mut fds = [-1i32; 1];
    let (_, nfds) = ours
        .recv_with_fd(&mut buf, &mut fds)
        .map_err(|e| format!("failed to receive descriptor from authopen: {e}"))?;

    // Let authopen finish once we've grabbed the fd; reap it off-thread.
    drop(ours);
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    if nfds < 1 || fds[0] < 0 {
        return Err(
            "Could not open the robot's storage. Authorization was denied, or the device is busy."
                .to_string(),
        );
    }

    Ok(unsafe { fs::File::from_raw_fd(fds[0]) })
}

fn copy_input<I: Read + SeekForward>(
    mut input: I,
    out: fs::File,
    bmap: Option<&Bmap>,
    total: u64,
    report: Box<dyn FnMut(u64, u64) + Send>,
) -> Result<(), String> {
    let sector = SectorWriter::new(out);
    let mut writer = ProgressWriter { inner: sector, written: 0, total, last_emit: 0, report };

    let result = match bmap {
        Some(b) => bmap_parser::copy(&mut input, &mut writer, b).map_err(|e| format!("flash failed: {e}")),
        None => bmap_parser::copy_nobmap(&mut input, &mut writer).map_err(|e| format!("flash failed: {e}")),
    };

    writer.flush().map_err(|e| format!("failed to flush: {e}"))?;
    // Zero-pad and write the final short block (raw devices need aligned writes).
    writer
        .inner
        .finish()
        .map_err(|e| format!("failed to finalize write: {e}"))?;
    result?;
    (writer.report)(total, total);
    Ok(())
}

// ---------------------------------------------------------------------------
// Elevated worker (real disk writes)
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "macos"))]
fn flash_elevated(
    app: &AppHandle,
    image_path: &str,
    bmap_path: Option<&str>,
    target: &str,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let progress = std::env::temp_dir().join(format!("reachy-flash-{}.progress", std::process::id()));
    let _ = fs::remove_file(&progress);

    let bmap_arg = bmap_path.unwrap_or("-");
    let progress_str = progress.to_string_lossy().to_string();

    // Tail the progress file while the elevated worker runs.
    let stop = Arc::new(AtomicBool::new(false));
    let poller = {
        let app = app.clone();
        let stop = stop.clone();
        let progress = progress.clone();
        thread::spawn(move || loop {
            if let Ok(content) = fs::read_to_string(&progress) {
                let content = content.trim();
                if let Some((w, t)) = content.split_once(' ') {
                    if let (Ok(w), Ok(t)) = (w.parse::<u64>(), t.parse::<u64>()) {
                        emit_progress(&app, "flashing", w, t);
                    }
                }
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(400));
        })
    };

    let status = run_elevated(&exe, image_path, bmap_arg, target, &progress_str);

    stop.store(true, Ordering::Relaxed);
    let _ = poller.join();

    let final_content = fs::read_to_string(&progress).unwrap_or_default();
    let _ = fs::remove_file(&progress);

    match status {
        Ok(true) => {
            emit_progress(app, "done", 1, 1);
            Ok(())
        }
        Ok(false) => {
            let err = worker_error(&final_content);
            if crate::images::looks_like_corrupt_image(&err) {
                crate::images::purge_cache();
                return Err(format!(
                    "{err}\n\nThe downloaded image was corrupt and has been removed. \
                     Relaunch to download a fresh copy, then flash again."
                ));
            }
            Err(err)
        }
        Err(e) => Err(e),
    }
}

#[cfg(not(target_os = "macos"))]
fn worker_error(final_content: &str) -> String {
    final_content
        .lines()
        .rev()
        .find(|l| l.starts_with("ERR "))
        .map(|l| l.trim_start_matches("ERR ").to_string())
        .unwrap_or_else(|| "flash failed or was cancelled".to_string())
}

#[cfg(target_os = "windows")]
fn run_elevated(
    exe: &Path,
    image: &str,
    bmap: &str,
    target: &str,
    progress: &str,
) -> Result<bool, String> {
    let args = format!(
        "'flash-worker','{}','{}','{}','{}'",
        image, bmap, target, progress
    );
    let ps = format!(
        "$p = Start-Process -FilePath '{}' -ArgumentList {} -Verb RunAs -Wait -PassThru; exit $p.ExitCode",
        exe.to_string_lossy(),
        args
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .output()
        .map_err(|e| format!("failed to request privileges: {e}"))?;
    Ok(out.status.success())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn run_elevated(_: &Path, _: &str, _: &str, _: &str, _: &str) -> Result<bool, String> {
    Err("elevated flashing is only implemented for macOS and Windows".to_string())
}

/// Entry point for the elevated child process (`flash-worker`).
/// Args: <image> <bmap|-> <target> <progress_file>
pub fn run_flash_worker(args: &[String]) {
    if args.len() < 4 {
        eprintln!("usage: flash-worker <image> <bmap|-> <target> <progress_file>");
        std::process::exit(2);
    }
    let (image, bmap, target, progress) = (&args[0], &args[1], &args[2], &args[3]);
    let bmap_opt = if bmap == "-" { None } else { Some(bmap.as_str()) };

    let progress_path = progress.clone();
    let report: Box<dyn FnMut(u64, u64) + Send> = Box::new(move |w, t| {
        let _ = fs::write(&progress_path, format!("{w} {t}"));
    });

    match do_flash(image, bmap_opt, target, report) {
        Ok(()) => {
            let _ = fs::write(progress, "DONE");
            std::process::exit(0);
        }
        Err(e) => {
            let _ = fs::write(progress, format!("ERR {e}"));
            std::process::exit(1);
        }
    }
}
