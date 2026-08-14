#!/usr/bin/env bash
# Copy the built Windows VST3 bundle into your Ableton (Wine) prefix's VST3 folder.
#
# Point it at the prefix Ableton runs in, e.g.:
#   WINEPREFIX="$HOME/.wine-ableton" ./deploy.sh
#
# Or skip this script entirely and just add target/bundled/ as a custom VST3
# folder in Ableton: Preferences > Plug-Ins > VST3 Plug-In Custom Folder.
set -euo pipefail

PREFIX="${WINEPREFIX:-$HOME/.wine}"
DEST="$PREFIX/drive_c/Program Files/Common Files/VST3"

BUNDLE="$(ls -d target/bundled/*.vst3 2>/dev/null | head -1 || true)"
if [[ -z "${BUNDLE:-}" ]]; then
  echo "No VST3 bundle found in target/bundled/. Run ./build.sh first." >&2
  exit 1
fi

if [[ ! -d "$PREFIX/drive_c" ]]; then
  echo "Wine prefix '$PREFIX' doesn't look right (no drive_c)." >&2
  echo "Set WINEPREFIX to the prefix Ableton runs in." >&2
  exit 1
fi

mkdir -p "$DEST"
rm -rf "$DEST/$(basename "$BUNDLE")"
cp -r "$BUNDLE" "$DEST/"
echo ">> Installed $(basename "$BUNDLE") -> $DEST"
echo ">> Rescan in Ableton (Preferences > Plug-Ins > Rescan) or restart Live."
