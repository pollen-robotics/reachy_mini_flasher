#!/usr/bin/env bash
#
# Fetch/build rpiboot (raspberrypi/usbboot) and stage the artifacts the flasher
# needs to expose the CM4 eMMC:
#   - the `rpiboot` binary
#   - the `mass-storage-gadget64` boot-files directory
#
# They are copied to src-tauri/binaries/rpiboot/ so they can be either bundled
# as Tauri resources (production) or picked up in dev via env overrides.
#
# macOS/Linux only. On Windows, install the RPiBoot GUI from
# https://github.com/raspberrypi/usbboot/raw/master/win32/rpiboot_setup.exe
# and copy rpiboot.exe + mass-storage-gadget64 into the same folder.
#
# Usage:
#   ./scripts/fetch-rpiboot.sh
#
# Prereqs (macOS):  brew install pkg-config git
#          (Linux):  sudo apt install libusb-1.0-0-dev pkg-config build-essential git
#
# On macOS, libusb is built from source and linked statically: the shipped
# rpiboot must not load a Homebrew dylib (users don't have one), and must run
# on macOS versions older than the build machine.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FLASHER_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST="$FLASHER_ROOT/src-tauri/binaries/rpiboot"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

LIBUSB_VERSION=1.0.29

if [ "$(uname -s)" = Darwin ]; then
  export MACOSX_DEPLOYMENT_TARGET=11.0

  echo "==> Building static libusb $LIBUSB_VERSION"
  curl -fsSL "https://github.com/libusb/libusb/releases/download/v$LIBUSB_VERSION/libusb-$LIBUSB_VERSION.tar.bz2" \
    | tar -xj -C "$WORK"
  (
    cd "$WORK/libusb-$LIBUSB_VERSION"
    ./configure --quiet --prefix="$WORK/libusb" --disable-shared --enable-static
    make -s -j"$(sysctl -n hw.ncpu)" install
  )
  export PKG_CONFIG_PATH="$WORK/libusb/lib/pkgconfig"
  export PKG_CONFIG_LIBDIR="$PKG_CONFIG_PATH"
  # Makes `pkg-config --libs` (what usbboot's Makefile calls) emit the static deps.
  sed -i '' 's/^Libs: \(.*\)$/Libs: \1 -lobjc -Wl,-framework,IOKit -Wl,-framework,CoreFoundation -Wl,-framework,Security/' \
    "$PKG_CONFIG_PATH/libusb-1.0.pc"
fi

echo "==> Cloning raspberrypi/usbboot"
git clone --depth 1 https://github.com/raspberrypi/usbboot.git "$WORK/usbboot"

echo "==> Building rpiboot"
make -C "$WORK/usbboot"

if [ "$(uname -s)" = Darwin ] && otool -L "$WORK/usbboot/rpiboot" | tail -n +2 | grep -Ev '^[[:space:]]*/(usr/lib|System/Library)/'; then
  echo "rpiboot links a library macOS doesn't ship (listed above)" >&2
  exit 1
fi

echo "==> Staging artifacts into $DEST"
mkdir -p "$DEST"
cp "$WORK/usbboot/rpiboot" "$DEST/rpiboot"
chmod +x "$DEST/rpiboot"
rm -rf "$DEST/mass-storage-gadget64"
# -L dereferences symlinks (e.g. bootfiles.bin -> ../firmware/bootfiles.bin) so
# the staged dir is self-contained and rpiboot can find the boot files.
cp -RL "$WORK/usbboot/mass-storage-gadget64" "$DEST/mass-storage-gadget64"

cat <<EOF

Done. Artifacts staged in:
  $DEST/rpiboot
  $DEST/mass-storage-gadget64/

For dev (npm run tauri:dev), export these so the app finds them without bundling:

  export REACHY_RPIBOOT_BIN="$DEST/rpiboot"
  export REACHY_RPIBOOT_DIR="$DEST/mass-storage-gadget64"

For production, add to src-tauri/tauri.conf.json under "bundle":

  "resources": { "binaries/rpiboot": "rpiboot" }

EOF
