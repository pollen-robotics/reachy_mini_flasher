//! Windows: one place for everything that shells out to PowerShell.
//!
//! Disk enumeration, driver queries and every privilege elevation go through
//! `powershell.exe`, and each of those had its own copy of the same incantation.
//! Centralizing them fixes three things that were wrong in all copies:
//!
//! - **no console flash**: `CREATE_NO_WINDOW`, otherwise a black console window
//!   pops on screen for each call - and detection polls every ~1.5s. The
//!   *elevated* children need `-WindowStyle Hidden` on top of that: they are
//!   started through ShellExecute, which cannot inherit our (absent) console
//!   and hands each one a fresh visible window otherwise (see `run_elevated`),
//! - **quoting**: paths were interpolated into single-quoted PowerShell strings
//!   raw, so a user folder containing `'` (`C:\Users\O'Brien`) built a broken
//!   script, and space-containing paths were split into several arguments,
//! - **cancelled UAC**: declining the elevation prompt is now reported as such
//!   (exit code 1223 = `ERROR_CANCELLED`) instead of a generic failure, and
//!   without depending on the localized error text.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::de::DeserializeOwned;

/// `CREATE_NO_WINDOW` - run the helper without allocating a console.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Exit code we make PowerShell return when the user declines the UAC prompt
/// (`ERROR_CANCELLED`). Matching on the exception text instead would break on
/// any non-English Windows.
const EXIT_CANCELLED: i32 = 1223;

/// A windowless `powershell.exe` invocation running `script`.
///
/// The script is passed via `-EncodedCommand` rather than `-Command`. As plain
/// text it has to survive two layers of command-line quoting - Rust's, then
/// powershell.exe's own - and a script containing double quotes does **not**
/// reliably come through intact. That silently stripped the quotes around a
/// path containing a space, so `rpiboot` was invoked as
/// `-d C:\Program` and exited immediately, having found no boot files.
///
/// Base64'd UTF-16LE has no such hazard: there is nothing in the encoded form
/// for either parser to interpret, so the script arrives byte for byte.
pub fn command(script: &str) -> Command {
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-EncodedCommand",
        &encode_command(script),
    ])
    .creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Encode a script the way `-EncodedCommand` expects: UTF-16LE, then base64.
fn encode_command(script: &str) -> String {
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    base64_encode(&bytes)
}

/// Minimal standard base64. Avoids a dependency for one call site.
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 0x3f] as char);
        out.push(TABLE[(n >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 0x3f] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 0x3f] as char } else { '=' });
    }
    out
}

/// Run a script and return its stdout. `what` names the operation in errors.
pub fn run(script: &str, what: &str) -> Result<String, String> {
    let out = command(script)
        .output()
        .map_err(|e| format!("failed to run powershell for {what}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Strip the `\\?\` extended-length prefix from a path.
///
/// Tauri's `resource_dir()` hands back verbatim paths on Windows, and almost
/// nothing outside the Win32 file APIs will take one: `cmd.exe` answers "The
/// system cannot find the path specified", and rpiboot - a Cygwin binary -
/// quietly fails to locate its boot files and exits immediately. Any path
/// leaving this process for another program has to go through here.
///
/// `\\.\` device paths (the flash target, `\\.\PhysicalDrive1`) are a *different*
/// prefix and must survive untouched - hence matching on `\\?\` exactly.
pub fn simplify(path: &Path) -> String {
    let s = path.to_string_lossy();
    // \\?\UNC\server\share -> \\server\share
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

/// Wrap a value in a PowerShell single-quoted string, escaping quotes.
///
/// Single-quoted strings are literal in PowerShell (no `$var` expansion), and
/// the only escape needed is doubling an embedded `'`.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Parse PowerShell JSON output that is *meant* to be a list.
///
/// `ConvertTo-Json -AsArray` only exists in PowerShell 6+; `powershell.exe` on
/// Windows 10/11 is Windows PowerShell 5.1, where that parameter is an error
/// and a single result serializes to a bare object instead of a one-element
/// array. So no caller passes `-AsArray`, and both shapes are accepted here.
pub fn json_array<T: DeserializeOwned>(text: &str, what: &str) -> Result<Vec<T>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }

    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("failed to parse {what} JSON: {e}"))?;

    let items = match value {
        serde_json::Value::Null => return Ok(Vec::new()),
        serde_json::Value::Array(items) => items,
        single => vec![single],
    };

    items
        .into_iter()
        .map(|v| serde_json::from_value(v).map_err(|e| format!("unexpected {what} JSON: {e}")))
        .collect()
}

/// What the user is told when they decline the UAC prompt.
///
/// Public so callers can tell "the user said no" apart from "the helper ran and
/// failed" without re-deriving it from the text: in the first case nothing
/// happened at all, so there is nothing to wait for afterwards.
pub const DENIED: &str = "Authorization was denied - approve the Windows prompt to continue.";

/// Exit code the shim uses when `Start-Process` failed for any reason *other*
/// than a declined prompt. Distinct from the child's own exit code, which comes
/// back on stdout.
const EXIT_SHIM_FAILED: i32 = 1;

/// Run `exe` elevated (UAC prompt) and wait for it, returning its exit code.
///
/// `args` is passed to the child verbatim as its command line, so quote
/// individual arguments with `"` inside it when they may contain spaces.
///
/// The child's exit code is reported on stdout rather than as the shim's own
/// exit code, so the two can never be confused: `Ok(n)` always means the
/// elevated program ran and returned `n`, and `Err` always means it never ran.
pub fn run_elevated(exe: &Path, args: &str, working_dir: Option<&Path>) -> Result<i32, String> {
    let mut script = format!(
        "$ErrorActionPreference='Stop'; try {{ $p = Start-Process -FilePath {} -Verb RunAs -WindowStyle Hidden -Wait -PassThru",
        quote(&simplify(exe))
    );
    if !args.is_empty() {
        script.push_str(&format!(" -ArgumentList {}", quote(args)));
    }
    if let Some(dir) = working_dir {
        script.push_str(&format!(" -WorkingDirectory {}", quote(&simplify(dir))));
    }
    // Only a *declined prompt* may be reported as cancelled. Every other
    // Start-Process failure - a missing executable, a malformed argument list,
    // a policy block - used to exit with the same code, so the user was told to
    // approve a prompt that had never appeared. Win32 says which via
    // ERROR_CANCELLED on the exception (PowerShell wraps it in an
    // InvalidOperationException, hence the InnerException hop); anything else
    // keeps its own message, the only description of the real fault we get.
    script.push_str(&format!(
        " }} catch {{ \
           $w = $_.Exception; \
           if ($w -isnot [System.ComponentModel.Win32Exception]) {{ $w = $w.InnerException }}; \
           if ($w -is [System.ComponentModel.Win32Exception] -and \
               $w.NativeErrorCode -eq {EXIT_CANCELLED}) {{ exit {EXIT_CANCELLED} }}; \
           [Console]::Error.Write($_.Exception.Message); \
           exit {EXIT_SHIM_FAILED} \
         }}; \
         Write-Output ('EXIT=' + [string][int]$p.ExitCode); exit 0"
    ));

    let out = command(&script)
        .output()
        .map_err(|e| format!("failed to request administrator privileges: {e}"))?;

    match out.status.code() {
        // The shim itself succeeded, so the child ran: its exit code is on
        // stdout. Take the last marker in case anything else printed first.
        Some(0) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout
                .lines()
                .filter_map(|line| line.trim().strip_prefix("EXIT="))
                .filter_map(|code| code.parse().ok())
                .next_back()
                .ok_or_else(|| {
                    format!(
                        "the elevated helper did not report an exit code: {}",
                        stdout.trim()
                    )
                })
        }
        Some(EXIT_CANCELLED) => Err(DENIED.to_string()),
        Some(_) => {
            let detail = String::from_utf8_lossy(&out.stderr);
            let detail = detail.trim();
            Err(if detail.is_empty() {
                "the elevated helper could not be started".to_string()
            } else {
                format!("the elevated helper could not be started: {detail}")
            })
        }
        None => Err("the elevated helper was terminated".to_string()),
    }
}

/// Create the file an elevated child will redirect its output into.
///
/// The name must not be predictable. The file is written by an **elevated**
/// process but has to live somewhere the non-elevated app can read it back, so
/// it sits in the user's temp dir - and a fixed name there is a pre-plantable
/// symlink: point it at a file only administrators may write, and our own UAC
/// prompt overwrites that on the planter's behalf. Hence a unique name, created
/// here with `CREATE_NEW` + `FILE_FLAG_OPEN_REPARSE_POINT`, which fails outright
/// rather than following whatever is already sitting at the path.
fn create_capture_file(what: &str) -> Result<PathBuf, String> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// `FILE_FLAG_OPEN_REPARSE_POINT` - do not traverse a symlink/junction.
    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    let stem: String = what
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir()
        .join(format!("reachy-{stem}-{}-{nonce:x}.log", std::process::id()));

    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(&path)
        .map_err(|e| format!("failed to create the {what} log file: {e}"))?;
    Ok(path)
}

/// Run `exe` elevated and capture everything it prints. Returns its exit code
/// and that output, and logs both.
///
/// `run_elevated` on its own loses the output entirely: the child's console is
/// hidden (`-WindowStyle Hidden`), so there is no window to read even in
/// principle, and `Start-Process -Verb RunAs` cannot be combined with output
/// redirection - different parameter sets. So the child is wrapped in a
/// `cmd.exe` whose own redirection lands everything in a file we read back.
///
/// This is not merely for debugging: what the program said is the only
/// explanation of a failure that exists, and a bare exit code says nothing a
/// user can act on.
///
/// `what` names the operation in the log and in the temp file name. `args` is
/// the child's own argument string, quoted by the caller.
pub fn run_elevated_captured(exe: &Path, args: &str, what: &str) -> Result<(i32, String), String> {
    let out_file = create_capture_file(what)?;

    let command = format!(
        r#"/c ""{}" {} > "{}" 2>&1""#,
        simplify(exe),
        args,
        simplify(&out_file)
    );
    crate::flash::log(&format!("{what}: cmd.exe {command}"));

    let code = match run_elevated(Path::new("cmd.exe"), &command, None) {
        Ok(code) => code,
        // The child never ran at all - a declined prompt, or the shim itself
        // failing. Nothing was written, but the file was created before we knew
        // that, and `?` here would orphan it in the user's temp dir on every
        // cancelled install.
        Err(e) => {
            let _ = std::fs::remove_file(&out_file);
            return Err(e);
        }
    };
    let output = std::fs::read_to_string(&out_file).unwrap_or_default();
    let _ = std::fs::remove_file(&out_file);
    crate::flash::log(&format!(
        "{what}: exit={code}\n--- {what} output ---\n{}\n--- end ---",
        output.trim()
    ));

    Ok((code, output))
}

/// The last `lines` non-blank lines of a captured output, joined for a
/// single-line error message. The tail is where the reason lives; everything
/// before it is progress chatter.
pub fn output_tail(output: &str, lines: usize) -> String {
    let kept: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    kept.iter()
        .rev()
        .take(lines)
        .rev()
        .copied()
        .collect::<Vec<_>>()
        .join(" / ")
}

#[cfg(test)]
mod tests {
    use super::{json_array, output_tail, quote};

    #[test]
    fn keeps_the_last_meaningful_lines_of_output() {
        let out = "starting\n\nstep one\n   \nstep two\nfailed: no device\n";
        assert_eq!(output_tail(out, 2), "step two / failed: no device");
        // Fewer lines than asked for is not an error.
        assert_eq!(output_tail("only line", 4), "only line");
        // A program that printed nothing must not produce a stray separator.
        assert_eq!(output_tail("", 4), "");
        assert_eq!(output_tail("\n  \n\n", 4), "");
    }

    #[derive(serde::Deserialize, PartialEq, Debug)]
    struct Row {
        n: u32,
    }

    #[test]
    fn encodes_commands_as_utf16le_base64() {
        // What `powershell -EncodedCommand` expects. Verified against
        // `[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes('...'))`.
        assert_eq!(super::encode_command("A"), "QQA=");
        assert_eq!(super::encode_command("AB"), "QQBCAA==");
        assert_eq!(super::encode_command("ABC"), "QQBCAEMA");
        // The case that broke rpiboot: a script carrying a quoted path.
        assert_eq!(super::encode_command("\"a b\""), "IgBhACAAYgAiAA==");
    }

    #[test]
    fn strips_the_verbatim_prefix() {
        use std::path::Path;
        // What Tauri's resource_dir() returns, and what broke rpiboot.
        assert_eq!(
            super::simplify(Path::new(r"\\?\C:\Program Files\app\rpiboot.exe")),
            r"C:\Program Files\app\rpiboot.exe"
        );
        assert_eq!(super::simplify(Path::new(r"\\?\UNC\srv\share\x")), r"\\srv\share\x");
        // Device paths use a different prefix and must be left alone - the
        // flash target is one of these.
        assert_eq!(super::simplify(Path::new(r"\\.\PhysicalDrive1")), r"\\.\PhysicalDrive1");
        assert_eq!(super::simplify(Path::new(r"C:\plain\path")), r"C:\plain\path");
    }

    #[test]
    fn quotes_and_escapes_paths() {
        assert_eq!(quote(r"C:\Program Files\rpiboot.exe"), r"'C:\Program Files\rpiboot.exe'");
        // A folder named after a user like O'Brien used to truncate the script.
        assert_eq!(quote(r"C:\Users\O'Brien\app.exe"), r"'C:\Users\O''Brien\app.exe'");
    }

    #[test]
    fn accepts_both_powershell_json_shapes() {
        // PowerShell 6+ / -AsArray, and the 5.1 multi-result shape.
        assert_eq!(json_array::<Row>(r#"[{"n":1},{"n":2}]"#, "test").unwrap(), vec![Row { n: 1 }, Row { n: 2 }]);
        // Windows PowerShell 5.1 collapses a single result to a bare object.
        assert_eq!(json_array::<Row>(r#"{"n":7}"#, "test").unwrap(), vec![Row { n: 7 }]);
        // No results at all.
        assert!(json_array::<Row>("", "test").unwrap().is_empty());
        assert!(json_array::<Row>("null", "test").unwrap().is_empty());
    }
}
