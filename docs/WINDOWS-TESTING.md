# Windows test procedure

Step-by-step validation of the Windows flash path on real hardware. Everything
in `docs/WINDOWS.md` items #1-#4 is implemented and compiles, but **none of it
has ever run against a robot** - this is the procedure that changes that.

Work through the phases in order. Each has a **Record** box; filling those in is
the point of the exercise, not a formality - two of them (the device
`InstanceId` in phase 3 and the disk `FriendlyName` in phase 4) are the inputs
we're missing to close item #4 and to confirm the driver binds the right PID.

---

## What you need

- A **Windows 10 or 11, x64** machine you can install software on and reboot.
- A **Reachy Mini Wireless** (or a bare CM4) and a USB cable.
- The test installer, from the PR's build artifact:
  ```powershell
  gh run download 30924011596 -n reachy-mini-flasher-windows-unsigned
  ```
  or the *Artifacts* section of
  <https://github.com/pollen-robotics/reachy_mini_flasher/actions/runs/30924011596>
- Roughly an hour, and a network connection (the OS image is ~1-2 GB).

### Ideally: a machine that has never had RPiBoot installed

The single most valuable thing this test can exercise is the **one-click driver
install**, and it only appears when WinUSB isn't already bound to the CM4. A
machine that has run Raspberry Pi's RPiBoot installer will sail past that screen
- which is a valid result worth confirming, but it is not the interesting one.

Phase 3 has instructions for unbinding the driver if you only have a machine
that's already set up.

---

## Safety

> [!WARNING]
> Flashing **erases the robot's eMMC completely**. That's the intent, but make
> sure the robot doesn't hold anything you want.

- **Unplug every other USB storage device before phase 5.** The app filters out
  the system/boot disk and matches on the mass-storage-gadget signature, but the
  fewer candidate disks exist, the smaller the consequence of a bad match. This
  is the one step where a defect destroys data that isn't the robot's.
- Before confirming the flash, **check the disk shown in the app is the robot**
  - the size should look like the eMMC (~16/32 GB), not like your SSD.
- The installer is **unsigned**. SmartScreen blocking it is expected, not a
  symptom.

---

## Phase 0 - Record the baseline

Run in a **normal** (non-elevated) PowerShell window, before installing anything.

```powershell
$PSVersionTable.PSVersion
```

```powershell
Get-ItemProperty 'HKCU:\Software\Raspberry Pi' -ErrorAction SilentlyContinue
```

```powershell
Get-PnpDevice -PresentOnly | Where-Object InstanceId -like 'USB\VID_0A5C*' | Select-Object Status, Class, Service, InstanceId
```

The first one matters more than it looks: `powershell.exe` should report **5.1**.
That's the whole reason `ConvertTo-Json -AsArray` was a fatal bug - it's a
PowerShell 6+ parameter, so disk enumeration failed on every Windows machine.
Seeing 5.1 here confirms the fix was necessary rather than theoretical.

> **Record**
> - PowerShell version: `________`
> - RPiBoot already installed? `yes / no` (path if yes: `________`)
> - Any `VID_0A5C` device present with nothing plugged in? `yes / no`

---

## Phase 1 - Install

The artifact contains two installers. **Test the NSIS one first** - it's what the
in-app updater uses, so it's the one that matters most.

```
Reachy Mini Flasher_0.2.3_x64-setup.exe     <- NSIS, test this
Reachy Mini Flasher_0.2.3_x64_en-US.msi     <- WiX, test after (see phase 6)
```

Don't install both at once; uninstall one before trying the other.

1. Run the setup. SmartScreen will block it → **More info** → **Run anyway**.
2. Launch the app from the Start menu.

Find where it landed - you'll need this path later:

```powershell
Get-ChildItem "$env:LOCALAPPDATA\Programs", "$env:ProgramFiles" -Filter 'Reachy Mini Flasher*' -ErrorAction SilentlyContinue | Select-Object FullName
```

Confirm the bundled helpers shipped with it (substitute your install dir):

```powershell
Get-ChildItem -Recurse '<install-dir>\rpiboot' | Select-Object Name, Length
```

You should see `rpiboot.exe`, `cygusb-1.0.dll`, `cygwin1.dll`,
`wdi-simple.exe`, and `mass-storage-gadget64\bootfiles.bin` at ~1.5 MB.

> [!IMPORTANT]
> `bootfiles.bin` must be **megabytes**, not 25 bytes. The git repo carries a
> symlink placeholder of that name; only the real payload works.

- [ ] App launches, window is 680x640, footer shows **0.2.3**
- [ ] All five bundled files present, `bootfiles.bin` ~1.5 MB
- [ ] **No black console window flashes** while the app sits on any screen

That last one is a fixed bug worth watching for specifically: detection polls
PowerShell every ~1.5 s, and before the fix each poll popped a console window.
Leave the app on the "looking for your Reachy" screen for a minute and watch.

---

## Phase 2 - Simulation, before touching hardware

This exercises the whole UI flow, the image download and the progress reporting
with **no robot and no disk writes**. If something is broken here, it's broken
everywhere, and it's much cheaper to find now.

```powershell
$env:REACHY_FLASHER_SIM = '1'
& '<install-dir>\Reachy Mini Flasher.exe'
```

Walk the wizard end to end: *Get started* → image download → connect steps → a
simulated Reachy appears after ~4 s → select → flash → progress → done.

- [ ] OS image downloads (progress + version shown), no error
- [ ] Simulated device appears and is selectable
- [ ] Flash progress runs and reaches **done**

Close the app and clear the variable before continuing:

```powershell
Remove-Item Env:\REACHY_FLASHER_SIM
```

> **Record**
> - Simulation completed? `yes / no`
> - OS version downloaded: `________`

---

## Phase 3 - The WinUSB driver (item #2)

### 3a. Put the robot in download mode

Follow the app's own connect instructions: disassemble to reach the switch, set
it to **DOWNLOAD**, connect USB, power on.

### 3b. Before doing anything in the app, capture the device

```powershell
Get-PnpDevice -PresentOnly | Where-Object InstanceId -like 'USB\VID_0A5C*' | Select-Object Status, Class, Service, InstanceId, FriendlyName
```

> [!IMPORTANT]
> **This is the single most important thing to record in the whole procedure.**
> The app binds the driver to PID **`0x2711`** (BCM2711) on the assumption that's
> what a Reachy Mini's CM4 enumerates as. If the `InstanceId` shows a different
> PID - `2712`, `2764`, `2763` - then the install binds the wrong device and
> `win_driver.rs` needs that PID added.

Typical driverless state: `Status` is `Error` or `Unknown`, `Service` is empty.
Bound state: `Service` is `WinUSB`.

> **Record**
> - Full `InstanceId`: `________________________`
> - `Status`: `________`  `Service`: `________`

### 3c. If the driver is already bound, and you want to test the install flow

In Device Manager, find the device → right-click → **Uninstall device** → tick
**Attempt to remove the driver for this device** → unplug/replug. Re-run 3b to
confirm `Service` is now empty.

### 3d. The app's reaction

Open the app (not in simulation) and walk to the waiting screen.

**Expected with no driver bound:**

- [ ] Title reads **"One-time driver setup"** - *not* "Looking for your Reachy"
- [ ] Copy says the robot is connected but Windows needs a USB driver
- [ ] The bottom button reads **"Install USB driver"** (not "Get the driver")

The button text distinguishes two cases: **"Install USB driver"** means
`wdi-simple.exe` was found and it can do it in-app; **"Get the driver"** means it
couldn't find the helper and is falling back to sending you to Raspberry Pi's
installer. In this build it should be the former - if it isn't, the bundled
resource didn't resolve.

If instead it says "Looking for your Reachy" forever while 3b clearly shows a
device, the PnP detection isn't matching - capture the `InstanceId` and stop.

### 3e. Install it

Click **Install USB driver**.

- [ ] A UAC prompt appears → approve
- [ ] Windows' driver installation dialog appears and completes
- [ ] Takes well under a minute

Then verify:

```powershell
Get-PnpDevice -PresentOnly | Where-Object InstanceId -like 'USB\VID_0A5C*' | Select-Object Status, Service, InstanceId
```

- [ ] `Service` is now **`WinUSB`**, `Status` is `OK`

Also worth testing once: click the button and **decline** the UAC prompt. The app
should say authorization was denied and stay usable, not hang or report a
generic failure.

> **Record**
> - Button shown: `Install USB driver / Get the driver`
> - `Service` after install: `________`
> - Declining UAC gives a sensible message? `yes / no`

---

## Phase 4 - rpiboot exposes the eMMC (item #4)

Once the driver is bound, the app runs `rpiboot` on its own to load the
mass-storage bootcode.

- [ ] A second UAC prompt appears (this one is rpiboot) → approve
- [ ] Within ~10-30 s the robot re-enumerates and the app finds it
- [ ] The app shows **"Reachy found"** with a plausible size (~16/32 GB)

**Immediately run this, whether or not the app found the disk:**

```powershell
Get-Disk | Select-Object Number, FriendlyName, BusType, Size, IsBoot, IsSystem
```

> [!IMPORTANT]
> The `FriendlyName` of the robot's disk is the **second thing we're missing**.
> `detect.rs::looks_like_reachy_disk` matches on substrings observed on macOS
> (`mmcblk`, `file-stor`, `rpi-msd`, `compute module`, `raspberry`). Windows may
> report something else entirely, in which case the app will never say "Reachy
> found" even though everything upstream worked.

**If the app is stuck on "Looking for your Reachy" but `Get-Disk` shows a new
disk:** that's exactly item #4, and it's a one-line fix once you send me the
`FriendlyName`. Not a failure of phases 1-3.

> **Record**
> - Robot disk `Number`: `____`
> - **`FriendlyName`: `________________________`**
> - `BusType`: `________`  `Size`: `________`
> - Did the app detect it by itself? `yes / no`

---

## Phase 5 - The flash and the volume lock (item #3)

> Unplug all other USB storage now. Confirm the disk in the app is the robot.

### 5a. Deliberately stress the lock first

This is the whole point of item #3. Before starting the flash:

1. Open **File Explorer** on the robot's newly-appeared drive letter (Windows
   will likely have offered to format it - dismiss that, don't format).
2. Leave that window open.
3. Start the flash in the app.

Expected: the app locks and dismounts the volumes out from under Explorer and
proceeds. Windows may briefly complain the drive was removed - that's correct
behaviour.

- [ ] Flash starts despite Explorer holding the drive
- [ ] **Or** a clear **"Storage is busy"** message naming the cause

Either outcome is acceptable. What is **not** acceptable is a flash that appears
to start and then fails partway, or that reports success on a robot that won't
boot - that would mean the lock didn't take.

### 5b. The write

- [ ] A UAC prompt appears (the elevated flash worker) → approve
- [ ] Progress advances smoothly to 100 %
- [ ] Completes without error

Note the wall-clock time; a ~2 GB sparse write over USB should be minutes, not
tens of minutes.

### 5c. After the write

```powershell
Get-Disk | Select-Object Number, FriendlyName, PartitionStyle, Size
```

- [ ] The robot's disk shows a partition table again (the guard's
      `IOCTL_DISK_UPDATE_PROPERTIES` on drop is what makes Windows re-read it)

> **Record**
> - Explorer open during flash → outcome: `proceeded / "Storage is busy" / other`
> - Flash result: `success / failure`
> - Duration: `________`
> - Any error text, verbatim: `________________________`

---

## Phase 6 - The robot actually boots

The real proof.

1. Unplug USB, set the switch back to normal, reassemble, power on.
2. The robot should boot ReachyMiniOS and come up on WiFi as usual.

- [ ] Robot boots
- [ ] Behaves like a freshly flashed unit

Then, if you have appetite left, uninstall and repeat **phase 1 only** with the
`.msi` to confirm the WiX installer also produces a working app. The rest doesn't
need repeating.

---

## If something fails: capturing diagnostics

The app is built with `windows_subsystem = "windows"`, so **it has no console and
its diagnostics go nowhere by default** - every `eprintln!` (the driver query
detail, volume enumeration warnings, ioctl failures) is silently discarded in a
release build.

To capture them, launch it with redirected handles - this works even without a
console:

```powershell
Start-Process -FilePath '<install-dir>\Reachy Mini Flasher.exe' -RedirectStandardError "$env:USERPROFILE\Desktop\flasher-err.txt" -RedirectStandardOutput "$env:USERPROFILE\Desktop\flasher-out.txt"
```

Use the app normally, close it, then send me `flasher-err.txt`.

> [!NOTE]
> This does **not** capture the elevated children (rpiboot, wdi-simple, the flash
> worker) - they're separate processes launched via `Start-Process -Verb RunAs`
> and only their exit code comes back. That's a known limitation.

### Running rpiboot by hand

If phase 4 fails, this is the fastest way to see rpiboot's real error. From an
**elevated** PowerShell:

```powershell
& '<install-dir>\rpiboot\rpiboot.exe' -d '<install-dir>\rpiboot\mass-storage-gadget64'
```

Its output goes to that console, so you'll see the actual cause rather than just
an exit code.

### Useful one-liners

```powershell
Get-PnpDevice -PresentOnly | Where-Object InstanceId -like 'USB\VID_0A5C*' | Format-List
```

```powershell
Get-Disk | Format-List Number, FriendlyName, BusType, Size, PartitionStyle, IsBoot, IsSystem
```

```powershell
Get-Volume | Select-Object DriveLetter, FileSystemLabel, FileSystem, Size
```

### What to send back

For any failure: the phase number, the verbatim error text from the app,
`flasher-err.txt`, and the output of the two `Record` items (`InstanceId` and
`FriendlyName`). That's enough to diagnose almost anything in this path.

---

## Resetting between runs

**Unbind the driver** (to re-test phase 3): Device Manager → the device →
*Uninstall device* → tick *Attempt to remove the driver*.

**Clear the downloaded OS image** (to re-test the download):

```powershell
Remove-Item -Recurse -Force "$env:APPDATA\com.pollen-robotics.reachy-mini-flasher\images" -ErrorAction SilentlyContinue
```

**Environment overrides**, if you ever need to point the app at artifacts other
than the bundled ones:

```powershell
$env:REACHY_RPIBOOT_BIN = '<dir>\rpiboot.exe'
$env:REACHY_RPIBOOT_DIR = '<dir>\mass-storage-gadget64'
$env:REACHY_WDI_SIMPLE_BIN = '<dir>\wdi-simple.exe'
```

---

## Summary sheet

| # | Check | Result |
|---|---|---|
| 0 | PowerShell version is 5.1 | |
| 1 | Installs, launches, bundled files present | |
| 1 | No console windows flashing | |
| 2 | Simulation flow completes | |
| 3 | **Device `InstanceId` (PID!)** | |
| 3 | "One-time driver setup" + "Install USB driver" shown | |
| 3 | Driver binds, `Service` = `WinUSB` | |
| 3 | Declined UAC handled cleanly | |
| 4 | rpiboot exposes the eMMC | |
| 4 | **Disk `FriendlyName`** | |
| 4 | App detects the disk by itself | |
| 5 | Volume lock beats an open Explorer window | |
| 5 | Flash completes | |
| 6 | Robot boots | |
| 6 | `.msi` installer also works | |
