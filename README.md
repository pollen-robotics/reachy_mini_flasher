# Reachy Mini Flasher

Cross-platform (Windows / macOS) desktop app to **reflash the ReachyMiniOS
image** onto a Reachy Mini Wireless, **over USB only**.

It rewrites the full OS image onto the CM4 eMMC (factory reset), used to recover
a broken installation.

> A full OS reflash can only be done over USB: the CM4 must enter
> `mass-storage-gadget` mode via rpiboot, and you cannot rewrite the running
> rootfs from the device itself. Remote (WiFi) OS reflash would require an
> A/B / recovery mechanism on the OS side - see
> [`docs/ota-recommendation.md`](docs/ota-recommendation.md).

## UX

One screen, one flow:

1. **Plug** the Reachy Mini (CM4) via USB - it is auto-detected.
2. Click **Flash Reachy Mini**.
3. Watch the **progress** (image download if needed, then flashing).
4. **Done** - unplug and reassemble.

## How it works

- **Detect** (`detect.rs`): a Reachy is recognized either as a mass-storage disk
  (RPi-MSD / Compute Module / File-Stor signature) once its eMMC is exposed, or
  in USB *download mode* (Broadcom vendor id `0x0a5c`) via `nusb`.
- **Prepare** (`rpiboot.rs`): when the CM4 is in download mode (eMMC not yet
  exposed), the app runs `rpiboot -d mass-storage-gadget64` elevated to load the
  mass-storage bootcode, mirroring the manual procedure. The eMMC then appears
  as a disk and detection flips to `ready`.
- **Image** (`images.rs`): the OS image is resolved automatically - newest
  `*.img(.gz)` in the local cache, otherwise downloaded from the latest
  `pollen-robotics/reachy-mini-os` GitHub release (with a matching `.bmap`).
- **Flash** (`flash.rs`): [`bmap-parser`](https://crates.io/crates/bmap-parser)
  sparse write + verify, with on-the-fly gz decompression. Raw disk writes run in
  an **elevated copy of the app** (`flash-worker` subcommand, macOS admin prompt /
  Windows UAC); the GUI tails a progress file.

## Development

```bash
npm install
npm run tauri:dev     # run the desktop app
npm run build         # typecheck + build the frontend
```

### rpiboot (required for real hardware)

Exposing the eMMC needs `rpiboot` + the `mass-storage-gadget64` boot files from
[raspberrypi/usbboot](https://github.com/raspberrypi/usbboot). Fetch/build them:

```bash
./scripts/fetch-rpiboot.sh   # macOS/Linux: clones, builds, stages artifacts
```

Then, for dev, point the app at them:

```bash
export REACHY_RPIBOOT_BIN="$PWD/src-tauri/binaries/rpiboot/rpiboot"
export REACHY_RPIBOOT_DIR="$PWD/src-tauri/binaries/rpiboot/mass-storage-gadget64"
```

For production, bundle the folder by adding to `src-tauri/tauri.conf.json`:

```json
"bundle": { "resources": { "binaries/rpiboot": "rpiboot" } }
```

On Windows, install the RPiBoot GUI
([rpiboot_setup.exe](https://github.com/raspberrypi/usbboot/raw/master/win32/rpiboot_setup.exe))
and copy `rpiboot.exe` + `mass-storage-gadget64` into `src-tauri/binaries/rpiboot/`.

### Simulation mode (test without a robot or root)

Set `REACHY_FLASHER_SIM=1` to fake a connected Reachy, generate a small local
image and flash it to a temp file - no hardware, no network, no admin prompt.
The full detect -> flash -> progress -> done flow runs end to end.

```bash
REACHY_FLASHER_SIM=1 npm run tauri:dev
```

## Safety

Raw disk writes are dangerous. The target is always the detected Reachy
mass-storage disk (never the host system disk), and real writes require an
explicit OS privilege prompt.
