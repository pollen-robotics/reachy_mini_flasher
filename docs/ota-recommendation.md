# Recommendation: robust OTA for the next Reachy Mini

**Audience**: Pollen Robotics firmware / OS team
**Author**: HF apps team
**Date**: 2026-07
**Status**: proposal, for discussion

---

## TL;DR

Today a full OS reflash of the Reachy Mini Wireless is only possible over **USB**
(rpiboot mass-storage mode). There is no way to reflash the OS image over WiFi,
because the running rootfs cannot overwrite itself and the OS ships no A/B or
recovery mechanism.

For the **next robot**, we recommend designing the OS image around a **robust
OTA stack** so a full OS update becomes a single, safe, remote operation with
automatic rollback. The USB flasher app we are building stays as the
last-resort recovery tool for a fully bricked unit.

Recommended stack:

1. **A/B redundancy** via Raspberry Pi **tryboot** (native bootloader A/B)
2. **RAUC** as the update client, in **adaptive/delta** mode (or **Rugix Ctrl** for best-in-class delta)
3. **Uptane** for update security and EU **Cyber Resilience Act (CRA)** compliance
4. **Keep the existing app/OS split** (apps + daemon update independently from the base OS)

---

## Problem statement

- The Wireless robot runs ReachyMiniOS from the CM4's **16 GB eMMC** (soldered, no removable SD).
- You cannot rewrite the rootfs while it is mounted and running.
- A power cut mid-flash bricks the unit.
- Consequence today: OS recovery requires physical access (open head, switch SW1 to DOWNLOAD, USB2 cable, rpiboot), which is not acceptable as the primary update path for end users.

## Where Reachy Mini stands today

The current architecture is **not naive**, it is **incomplete on the base OS layer**:

| Layer | Current mechanism | Remote-updatable? |
|-------|-------------------|-------------------|
| HF apps | Installed per-app, updated via API | Yes |
| Daemon package | `pip install reachy_mini` + service restart | Yes (WiFi/BLE) |
| **Base OS image** | **rpiboot + bmap over USB** | **No** |

The app/daemon separation already mirrors the "container app layer" of modern
OTA designs. The missing piece is purely the **OS-image update layer**.

## State of the art (August 2026)

A/B redundancy is now **table stakes**, not the frontier. The current SOTA
stacks four layers:

### 1. A/B redundant partitions (reliability baseline)
Two system slots; write the inactive one, switch boot pointer, reboot, auto
rollback on failure. Production-ready engines: **RAUC** (cleanest architecture),
**Mender** (turn-key + fleet server), **SWUpdate** (most flexible).

### 2. Delta / adaptive updates (bandwidth, the real differentiator)
Transfer only what changed instead of full multi-GB images:
- **Rugix Ctrl** (Rust, memory-safe): content-defined chunking + delta compression, best-in-class efficiency, single self-contained update file.
- **RAUC adaptive updates** + HTTP range streaming (casync).
- SWUpdate via zchunk/casync.

Critical for a consumer robot updating over home WiFi.

### 3. Uptane (security + compliance)
TUF-based framework from automotive, now applied to robotics and ROS 2.
Dual **Image + Director** repositories, protection against rollback / freeze /
mix-and-match attacks, signed bundles. The EU **Cyber Resilience Act** makes
secure OTA effectively mandatory - relevant for an EU manufacturer.

### 4. OS/app separation (container / image-based)
libostree, balenaOS (Docker on device), Torizon. Base OS moves rarely (A/B),
apps update more often and independently. Reachy already does a version of this.

## Recommendation for the next robot

| Priority | Choice | Rationale |
|----------|--------|-----------|
| P0 | **A/B via tryboot** | Native to the RPi bootloader, atomic, auto-rollback, power-cut safe |
| P0 | **RAUC** update client | Cleanest slot model, U-Boot/tryboot integration, signed bundles |
| P1 | **Adaptive/delta** (RAUC adaptive or **Rugix Ctrl**) | Cut OTA size from GBs to MBs over home WiFi |
| P1 | **Uptane** signed bundles + image/director repos | Security + CRA compliance |
| P2 | **Formalize app-in-container layer** | Already partly there; decouple app cadence from OS |

Design implications (decide **before** finalizing the image, hardest to change later):

- Partition layout: 2x rootfs slots + shared data partition (16 GB eMMC has room).
- Bootloader: enable **tryboot** with `autoboot.txt` (`[all]` / `[tryboot]`).
- Image build: switch to a build system with mature OTA layers (Yocto + meta-rauc, or Buildroot).
- Distribution: an image/bundle server (signed). Hugging Face could host the endpoint.

## Role of the USB flasher app

Even with the best OTA, USB reflash never fully disappears:

- **Before OTA exists**: it is the only OS recovery path.
- **After OTA exists**: it becomes the **last-resort recovery** for a fully
  bricked unit (both slots dead, corrupted bootloader), which no over-the-air
  system can fix.

So the app is worth building now regardless of the OTA decision.

## References

- RAUC - https://rauc.io / https://github.com/rauc/rauc
- Rugix Ctrl (OTA engine comparison, 2026) - https://rugix.org/blog/2026-02-28-ota-update-engines-compared/
- Mender / SWUpdate comparison (2026) - https://proteanos.com/doc/ota-updates-rauc-swupdate-mender-2026/
- Uptane standard - https://uptane.org/docs/latest/standard/uptane-standard
- Raspberry Pi tryboot / A/B - Raspberry Pi bootloader documentation
