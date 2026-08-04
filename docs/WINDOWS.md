# Windows support - status & remaining work

The flasher is written to be cross-platform and **already has Windows code paths
for every OS-specific operation**. It compiles on Windows and produces `.msi` /
`.nsis` installers, and the **simulation flow** (`REACHY_FLASHER_SIM=1`) works.

However, **the real end-to-end flash has never been tested on Windows**, and a
few concrete gaps remain before it can be considered working. This document
tracks them.

> **TL;DR** - macOS is release-ready (see `.github/workflows/release-macos.yml`).
> Items **#2 (driver)** and **#3 (volume lock)** are now implemented; **#1
> (bundling)** and **#4 (disk signature)** remain, and nothing here has been run
> against real hardware yet.

---

## What already works on Windows

| Concern | Where | Status |
|---|---|---|
| Disk enumeration | `src-tauri/src/disks.rs` (`Get-Disk` via PowerShell, `\\.\PhysicalDriveN`, filters system disk) | ✅ implemented |
| Download-mode detection | `src-tauri/src/detect.rs` (`nusb`, Broadcom VID `0x0a5c`, + PnP fallback) | ✅ implemented |
| WinUSB driver binding | `src-tauri/src/win_driver.rs` (libwdi `wdi-simple.exe`, in-app) | ⚠️ implemented, untested |
| Volume lock / dismount | `src-tauri/src/win_volume.rs` (FSCTL lock + dismount + delete layout) | ⚠️ implemented, untested |
| Elevated flashing | `src-tauri/src/flash.rs` (`Start-Process -Verb RunAs` → `flash-worker`, `SectorWriter` block alignment) | ⚠️ implemented, untested |
| Elevated rpiboot | `src-tauri/src/rpiboot.rs` (`Start-Process -Verb RunAs`) | ⚠️ implemented, needs bundling |
| Bundle targets | `src-tauri/tauri.conf.json` (`msi`, `nsis`) | ✅ configured |
| Compile coverage | `.github/workflows/ci.yml` (`rust-windows` job) | ✅ clippy + tests on `windows-latest` |

Until the `rust-windows` CI job was added, **none of the Windows code was ever
compiled by anything** - the macOS job `cfg`s it all out. Treat "it builds" as
new information, not a baseline.

---

## Bugs found while implementing #2/#3

Three defects in the pre-existing Windows paths, all of which would have made a
real flash fail regardless of the items below:

- **`disks.rs` used `ConvertTo-Json -AsArray`.** That parameter only exists in
  PowerShell 6+; `powershell.exe` on Windows 10/11 is Windows PowerShell 5.1,
  where it is a hard error - so `Get-Disk` failed every time and **no disk was
  ever detected on Windows**. Fixed by dropping the flag and accepting both the
  array and the bare-object shape (`win_ps::json_array`).
- **Elevation quoted arguments wrongly.** `-ArgumentList 'a','b'` split
  space-containing paths (the image lives under `C:\Users\<name>\AppData\...`)
  into several arguments, and a path containing `'` (`C:\Users\O'Brien`) built a
  syntactically broken script. Fixed in `win_ps::run_elevated`.
- **Every PowerShell call flashed a console window.** Detection polls once per
  ~1.5s, so the app popped a black console at the user continuously. Fixed with
  `CREATE_NO_WINDOW` in `win_ps::command`.

---

## Remaining work

### 1. Bundle `rpiboot.exe` + `mass-storage-gadget64` - ✅ implemented (needs one CI run to confirm)

`scripts/fetch-rpiboot.sh` is bash-only and builds `rpiboot` from source. On
Windows there is no build worth doing: `rpiboot.exe` is a prebuilt **Cygwin**
binary that is *not* checked into raspberrypi/usbboot - it only exists inside
`rpiboot_setup.exe`.

**Implemented** as `scripts/fetch-rpiboot.ps1`, which downloads that installer
from a **pinned** upstream release (`windows-v1.1`, whose notes call out CM4
SDRAM support), unpacks it with 7-Zip and stages into
`src-tauri/binaries/rpiboot/`:

| File | Why |
|---|---|
| `rpiboot.exe` | the loader |
| `cygusb-1.0.dll`, `cygwin1.dll` | Cygwin runtime - `rpiboot.exe` won't start without both |
| `mass-storage-gadget64/` | boot files, including the **real** `bootfiles.bin` |
| `wdi-simple.exe` | libwdi helper item #2 uses to bind the WinUSB driver |

Two traps that cost real debugging time if missed, both now asserted by the
script:

- **the Cygwin DLLs.** Copying `rpiboot.exe` alone yields a binary that dies on
  launch with no useful message.
- **`bootfiles.bin`.** In the git repo it's a 25-byte symlink placeholder to
  `../firmware/bootfiles.bin`; only the installer carries the real payload. Feed
  rpiboot the placeholder and it fails with `No 'bootcode' files found`. The
  script size-checks it and refuses to produce a broken bundle.

`.github/workflows/release-windows.yml` calls it before `tauri build` and
produces unsigned `.msi` + NSIS installers. It has a `stage_only` dispatch input
that runs the staging and verification alone, so an upstream layout change can be
caught without cutting a release.

Note the resource layout: `win_driver.rs` looks for `wdi-simple.exe` directly in
the bundled `rpiboot/` dir, **not** in a `redist/` subdirectory as the upstream
installer lays it out.

Independently of all this, `rpiboot.rs` and `win_driver.rs` also fall back to an
existing RPiBoot installation (`HKCU\Software\Raspberry Pi`, else
`%ProgramFiles%\Raspberry Pi`), so a machine that ran the official installer
works without any bundling at all.

### 2. WinUSB driver for the CM4 - ✅ implemented (untested on hardware)

In download mode the CM4 enumerates as a Broadcom USB device. On Windows it is
**not usable until a WinUSB driver is bound to it**. Without it:

- `nusb` does not enumerate the device → download mode is never detected,
- `rpiboot.exe` cannot talk to it.

**Implemented** in `src-tauri/src/win_driver.rs`, taking the same route as
Raspberry Pi's own `rpiboot_setup.exe`: run **libwdi**'s `wdi-simple.exe` to bind
WinUSB to the device.

```
wdi-simple.exe -n "Raspberry Pi USB boot" -v 0x0a5c -p 0x2711 -t 0
```

- Only `0x2711` (BCM2711, the CM4) is bound. Upstream binds all four boot PIDs
  (`0x2763`/`0x2764`/`0x2711`/`0x2712`); doing the same would mean four UAC
  prompts for three chips Reachy Mini doesn't have.
- `winusb_status` reports `{device_present, driver_ok, can_install}` by querying
  the PnP tree (`Get-PnpDevice`, `InstanceId -like 'USB\VID_0A5C&PID_27*'`), and
  treats a `WinUSB`/`libusb*` service as bound - so a user who already ran Zadig
  or the RPiBoot installer is not asked again.
- `detect.rs` consults the PnP tree as well, so a driverless robot reads as
  "connected, needs a driver" instead of "nothing plugged in".
- The connect screen turns that into an **Install USB driver** button; when
  `wdi-simple.exe` isn't available it degrades to a link to the official
  installer instead.

Still needs consent (a UAC prompt plus Windows' driver dialog) - that part is
irreducible, but it is now one click inside the app.

### 3. Raw-disk writes need volume lock / dismount - ✅ implemented (untested on hardware)

**Implemented** in `src-tauri/src/win_volume.rs`, called from `do_flash()` right
where macOS calls `diskutil unmountDisk`:

1. enumerate volumes with `FindFirstVolumeW`/`FindNextVolumeW` and keep those
   whose `IOCTL_STORAGE_GET_DEVICE_NUMBER` matches the target drive - volume GUID
   paths rather than drive letters, because the CM4's Linux partitions get no
   letter yet still hold the disk,
2. `FSCTL_LOCK_VOLUME` (retried for ~3s: a freshly appeared disk is usually being
   poked by Explorer or an antivirus scan),
3. `FSCTL_DISMOUNT_VOLUME`,
4. **keep the handles open** for the whole write - a guard object held by
   `do_flash`, since closing a handle releases the lock and lets Windows remount
   mid-flash,
5. `IOCTL_DISK_DELETE_DRIVE_LAYOUT` so there is no partition table left to
   auto-mount,
6. on drop: release the locks and `IOCTL_DISK_UPDATE_PROPERTIES` so Explorer
   re-reads the newly written table.

Uses the `windows` crate (target-gated in `Cargo.toml`). A volume that refuses to
dismount fails the flash with a message naming the cause, rather than corrupting
the write.

### 4. Verify the exposed-eMMC disk description

`detect.rs::looks_like_reachy_disk` matches on the disk description. The
signatures (`mmcblk`, `file-stor`, `rpi-msd`, …) were observed on macOS. On
Windows, `Get-Disk`'s `FriendlyName` for the same gadget may differ.

**TODO:** plug a CM4 in download mode on Windows, run
`Get-Disk | Select Number,FriendlyName,BusType`, and add the observed
`FriendlyName` to `looks_like_reachy_disk` if it isn't already matched.

---

## Suggested order of attack

1. **#1 bundling** (`fetch-rpiboot.ps1` + Windows release workflow) - the only
   thing standing between the current code and an installer on a Windows box.
2. **#4 disk match** - quick, but needs the hardware.
3. **Validate #2 and #3 on real hardware.** They are written and they compile,
   which is not the same as working.

### How to test #2/#3 today, without waiting for #1

On a Windows machine with the repo checked out:

```
npm install
npm run tauri:dev
```

Point the app at an existing RPiBoot installation instead of bundled resources:

```
set REACHY_RPIBOOT_BIN=C:\Program Files\Raspberry Pi\rpiboot.exe
set REACHY_RPIBOOT_DIR=C:\Program Files\Raspberry Pi\mass-storage-gadget64
set REACHY_WDI_SIMPLE_BIN=C:\Program Files\Raspberry Pi\redist\wdi-simple.exe
```

(`win_driver.rs` finds all three on its own if RPiBoot is installed; the
overrides are for testing a *partial* setup - e.g. deliberately unbinding the
driver in Device Manager to exercise the install flow.)

What to check, in order:

- with the driver unbound, the connect screen says **"One-time driver setup"**
  and offers **Install USB driver** - not "no robot found",
- after installing, the CM4 appears and rpiboot exposes the eMMC,
- `Get-Disk | Select Number,FriendlyName,BusType` - feed the `FriendlyName` into
  item #4,
- during the flash, open Explorer on the robot's drive first: the lock/dismount
  should either win or produce "Storage is busy", never a partial write.

---

## macOS note (for contrast)

macOS needs **none** of the above:
- `rpiboot` is built from source by `scripts/fetch-rpiboot.sh` and bundled.
- Raw disk access uses Apple's `authopen` (setuid) to get the fd, bypassing the
  Full Disk Access requirement; `diskutil unmountDisk` handles the dismount.
- Only signing/notarization is optional (secrets in the release workflow).
