# Windows support - status & remaining work

The flasher is written to be cross-platform and **already has Windows code paths
for every OS-specific operation**. It compiles on Windows and produces `.msi` /
`.nsis` installers, and the **simulation flow** (`REACHY_FLASHER_SIM=1`) works.

However, **the real end-to-end flash has never been tested on Windows**, and a
few concrete gaps remain before it can be considered working. This document
tracks them.

> **TL;DR** - macOS is release-ready (see `.github/workflows/release-macos.yml`).
> Windows builds, but the real-hardware flash needs the four items below.

---

## What already works on Windows

| Concern | Where | Status |
|---|---|---|
| Disk enumeration | `src-tauri/src/disks.rs` (`Get-Disk` via PowerShell, `\\.\PhysicalDriveN`, filters system disk) | ✅ implemented |
| Download-mode detection | `src-tauri/src/detect.rs` (`nusb`, Broadcom VID `0x0a5c`) | ✅ implemented (driver caveat, see #2) |
| Elevated flashing | `src-tauri/src/flash.rs` (`Start-Process -Verb RunAs` → `flash-worker`, `SectorWriter` block alignment) | ⚠️ implemented, untested |
| Elevated rpiboot | `src-tauri/src/rpiboot.rs` (`Start-Process -Verb RunAs`) | ⚠️ implemented, needs bundling + driver |
| Bundle targets | `src-tauri/tauri.conf.json` (`msi`, `nsis`) | ✅ configured |

---

## Remaining work

### 1. Bundle `rpiboot.exe` + `mass-storage-gadget64` for Windows

`scripts/fetch-rpiboot.sh` is bash-only (macOS/Linux). It builds `rpiboot` from
source and stages it into `src-tauri/binaries/rpiboot/`, which is then bundled
via the `bundle.resources` entry in `tauri.conf.json`.

On Windows there is no build step - the artifacts must be fetched from the
prebuilt RPiBoot installer:

- Installer: <https://github.com/raspberrypi/usbboot/raw/master/win32/rpiboot_setup.exe>
- Copy `rpiboot.exe` **and** the `mass-storage-gadget64/` directory into
  `src-tauri/binaries/rpiboot/`.

**TODO:** add `scripts/fetch-rpiboot.ps1` (PowerShell) that downloads/extracts
these into `src-tauri/binaries/rpiboot/`, and call it from the Windows release
workflow before `tauri build`. `rpiboot.rs` already looks for `rpiboot.exe` in
the bundled `rpiboot/` resource dir, so no Rust change is needed.

### 2. WinUSB driver for the CM4 (biggest blocker)

In download mode the CM4 enumerates as a Broadcom USB device. On Windows it is
**not usable until a WinUSB driver is bound to it** (installed by the RPiBoot
setup above, or manually via Zadig). Without it:

- `nusb` may not enumerate the device → download mode is never detected.
- `rpiboot.exe` cannot talk to it → "rpiboot failed (is the WinUSB driver installed?)".

**TODO (choose one):**
- Ship the RPiBoot driver installer and run it (or instruct the user to) on
  first launch, **or**
- Document a manual Zadig step in the app's troubleshooting for the connect
  screen.

This step inherently requires user consent (driver install) and is the main
reason Windows can't be fully silent like macOS.

### 3. Raw-disk writes need volume lock / dismount

The Windows path opens `\\.\PhysicalDriveN` read/write directly:

```rust
// src-tauri/src/flash.rs -> open_target()
fs::OpenOptions::new().read(true).write(true).open(Path::new(target))
```

On Windows, writing to a physical drive that has **mounted volumes** is blocked
(sharing violation / access denied) or silently fails once you hit a mounted
region. The exposed eMMC has partitions Windows auto-mounts, so this will very
likely fail on real hardware.

**TODO:** before writing, for each volume on the target disk:
1. open the volume (`\\.\X:`),
2. `FSCTL_LOCK_VOLUME`,
3. `FSCTL_DISMOUNT_VOLUME`,
4. keep the handles open for the duration of the write,

and ideally `IOCTL_DISK_DELETE_DRIVE_LAYOUT` on the physical drive first. This is
a Windows-only addition in `flash.rs` (behind `#[cfg(target_os = "windows")]`),
using the `windows` crate. balenaEtcher's `etcher-sdk` does exactly this and is a
good reference.

### 4. Verify the exposed-eMMC disk description

`detect.rs::looks_like_reachy_disk` matches on the disk description. The
signatures (`mmcblk`, `file-stor`, `rpi-msd`, …) were observed on macOS. On
Windows, `Get-Disk`'s `FriendlyName` for the same gadget may differ.

**TODO:** plug a CM4 in download mode on Windows, run
`Get-Disk | Select Number,FriendlyName,BusType`, and add the observed
`FriendlyName` to `looks_like_reachy_disk` if it isn't already matched.

---

## Suggested order of attack

1. **#1 bundling** (`fetch-rpiboot.ps1` + Windows release workflow) - unblocks
   getting a real installer onto a Windows box.
2. **#2 driver** - decide install-vs-document, otherwise nothing downstream works.
3. **#4 disk match** - quick, needs the hardware from #2.
4. **#3 volume lock/dismount** - the actual write fix; test with #1-#3 in place.

All four should be validated on a real Windows machine (or a Windows CI runner
with a device attached) - they fail silently or differently when guessed at
blind.

---

## macOS note (for contrast)

macOS needs **none** of the above:
- `rpiboot` is built from source by `scripts/fetch-rpiboot.sh` and bundled.
- Raw disk access uses Apple's `authopen` (setuid) to get the fd, bypassing the
  Full Disk Access requirement; `diskutil unmountDisk` handles the dismount.
- Only signing/notarization is optional (secrets in the release workflow).
