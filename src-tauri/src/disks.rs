//! Native, cross-platform enumeration of physical disks that are safe flash
//! targets.
//!
//! We deliberately do NOT use a generic crate here: `rs-drivelist` has no macOS
//! support yet, and disk selection is the single most dangerous operation in
//! this app (writing to the wrong disk destroys the host system). So we query
//! the OS tools directly and only ever surface **external / removable**,
//! **non-system** disks as candidates.

use serde::Serialize;

/// A physical disk presented to the UI as a potential flash target.
#[derive(Serialize, Clone, Debug)]
pub struct DiskInfo {
    /// Path to open for raw writing (e.g. `/dev/rdisk4`, `\\.\PhysicalDrive2`).
    pub device: String,
    /// Human-facing path (e.g. `/dev/disk4`), also used for unmounting.
    pub display_device: String,
    /// Model / media name.
    pub description: String,
    /// Size in bytes.
    pub size: u64,
    pub is_removable: bool,
    pub is_external: bool,
    /// Never a valid target. Present only so the UI can hard-block it.
    pub is_system: bool,
}

/// Returns all external/removable, non-system physical disks.
pub fn list() -> Result<Vec<DiskInfo>, String> {
    platform::list()
}

#[cfg(target_os = "macos")]
mod platform {
    use super::DiskInfo;
    use std::io::Cursor;
    use std::process::Command;

    fn diskutil_plist(args: &[&str]) -> Result<plist::Value, String> {
        let out = Command::new("diskutil")
            .args(args)
            .output()
            .map_err(|e| format!("failed to run diskutil: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "diskutil {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        plist::Value::from_reader(Cursor::new(out.stdout))
            .map_err(|e| format!("failed to parse diskutil plist: {e}"))
    }

    pub fn list() -> Result<Vec<DiskInfo>, String> {
        // `external physical` already excludes the internal system disk.
        let root = diskutil_plist(&["list", "-plist", "external", "physical"])?;
        let ids: Vec<String> = root
            .as_dictionary()
            .and_then(|d| d.get("WholeDisks"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_string().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let mut disks = Vec::new();
        for id in ids {
            match disk_info(&id) {
                Ok(info) => disks.push(info),
                Err(e) => eprintln!("skipping disk {id}: {e}"),
            }
        }
        Ok(disks)
    }

    fn disk_info(id: &str) -> Result<DiskInfo, String> {
        let dev = format!("/dev/{id}");
        let info = diskutil_plist(&["info", "-plist", &dev])?;
        let dict = info
            .as_dictionary()
            .ok_or_else(|| "unexpected diskutil info format".to_string())?;

        let size = dict
            .get("Size")
            .or_else(|| dict.get("TotalSize"))
            .and_then(|v| v.as_unsigned_integer())
            .unwrap_or(0);

        let description = dict
            .get("MediaName")
            .or_else(|| dict.get("IORegistryEntryName"))
            .and_then(|v| v.as_string())
            .unwrap_or("Unknown device")
            .trim()
            .to_string();

        let internal = dict.get("Internal").and_then(|v| v.as_boolean()).unwrap_or(false);
        let removable = dict
            .get("RemovableMedia")
            .or_else(|| dict.get("Removable"))
            .and_then(|v| v.as_boolean())
            .unwrap_or(false);

        Ok(DiskInfo {
            device: format!("/dev/r{id}"),
            display_device: dev,
            description,
            size,
            is_removable: removable,
            is_external: !internal,
            // We queried `external physical`, so these are never the system disk.
            is_system: false,
        })
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::DiskInfo;
    use crate::win_ps;

    pub fn list() -> Result<Vec<DiskInfo>, String> {
        // NOTE: no `-AsArray`. That parameter only exists in PowerShell 6+, and
        // `powershell.exe` on Windows 10/11 is Windows PowerShell 5.1, where it
        // is a hard error - which made this whole function fail, so no disk was
        // ever detected on Windows. `win_ps::json_array` handles the bare-object
        // shape 5.1 emits for a single result instead.
        let script = "Get-Disk | ForEach-Object { [PSCustomObject]@{ Number = $_.Number; FriendlyName = $_.FriendlyName; Size = [uint64]$_.Size; BusType = [string]$_.BusType; IsBoot = [bool]$_.IsBoot; IsSystem = [bool]$_.IsSystem } } | ConvertTo-Json -Compress";

        let out = win_ps::run(script, "Get-Disk")?;
        let parsed: Vec<RawDisk> = win_ps::json_array(&out, "Get-Disk")?;

        Ok(parsed
            .into_iter()
            .map(|d| {
                let is_usb = d.bus_type.eq_ignore_ascii_case("USB");
                let is_system = d.is_boot || d.is_system;
                DiskInfo {
                    device: format!("\\\\.\\PhysicalDrive{}", d.number),
                    display_device: format!("PhysicalDrive{}", d.number),
                    description: d.friendly_name.unwrap_or_else(|| "Unknown device".into()),
                    size: d.size,
                    is_removable: is_usb,
                    is_external: is_usb,
                    is_system,
                }
            })
            // Never surface the system/boot disk as a target.
            .filter(|d| !d.is_system)
            .collect())
    }

    #[derive(serde::Deserialize)]
    struct RawDisk {
        #[serde(rename = "Number")]
        number: u32,
        #[serde(rename = "FriendlyName")]
        friendly_name: Option<String>,
        #[serde(rename = "Size")]
        size: u64,
        #[serde(rename = "BusType")]
        bus_type: String,
        #[serde(rename = "IsBoot")]
        is_boot: bool,
        #[serde(rename = "IsSystem")]
        is_system: bool,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::DiskInfo;

    pub fn list() -> Result<Vec<DiskInfo>, String> {
        Err("disk enumeration is only implemented for macOS and Windows".into())
    }
}
