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

/// A `powershell.exe -Command <script>` invocation, windowless.
pub fn command(script: &str) -> Command {
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW);
    cmd
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
        quote(&exe.to_string_lossy())
    );
    if !args.is_empty() {
        script.push_str(&format!(" -ArgumentList {}", quote(args)));
    }
    if let Some(dir) = working_dir {
        script.push_str(&format!(
            " -WorkingDirectory {}",
            quote(&dir.to_string_lossy())
        ));
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
