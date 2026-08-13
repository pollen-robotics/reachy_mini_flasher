//! Windows: one place for everything that shells out to PowerShell.
//!
//! Disk enumeration, driver queries and every privilege elevation go through
//! `powershell.exe`, and each of those had its own copy of the same incantation.
//! Centralizing them fixes three things that were wrong in all copies:
//!
//! - **no console flash**: `CREATE_NO_WINDOW`, otherwise a black console window
//!   pops on screen for each call - and detection polls every ~1.5s,
//! - **quoting**: paths were interpolated into single-quoted PowerShell strings
//!   raw, so a user folder containing `'` (`C:\Users\O'Brien`) built a broken
//!   script, and space-containing paths were split into several arguments,
//! - **cancelled UAC**: declining the elevation prompt is now reported as such
//!   (exit code 1223 = `ERROR_CANCELLED`) instead of a generic failure, and
//!   without depending on the localized error text.

use std::os::windows::process::CommandExt;
use std::path::Path;
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

/// Run `exe` elevated (UAC prompt) and wait for it, returning its exit code.
///
/// `args` is passed to the child verbatim as its command line, so quote
/// individual arguments with `"` inside it when they may contain spaces.
pub fn run_elevated(exe: &Path, args: &str, working_dir: Option<&Path>) -> Result<i32, String> {
    let mut script = format!(
        "$ErrorActionPreference='Stop'; try {{ $p = Start-Process -FilePath {} -Verb RunAs -Wait -PassThru",
        quote(&simplify(exe))
    );
    if !args.is_empty() {
        script.push_str(&format!(" -ArgumentList {}", quote(args)));
    }
    if let Some(dir) = working_dir {
        script.push_str(&format!(" -WorkingDirectory {}", quote(&simplify(dir))));
    }
    // A declined UAC prompt makes Start-Process throw; report it distinctly.
    script.push_str(&format!(" }} catch {{ exit {EXIT_CANCELLED} }}; exit $p.ExitCode"));

    let out = command(&script)
        .output()
        .map_err(|e| format!("failed to request administrator privileges: {e}"))?;

    match out.status.code() {
        Some(EXIT_CANCELLED) => {
            Err("Authorization was denied - approve the Windows prompt to continue.".to_string())
        }
        Some(code) => Ok(code),
        None => Err("the elevated helper was terminated".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{json_array, quote};

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
