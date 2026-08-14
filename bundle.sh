#!/usr/bin/env bash
# Build a plugin crate and package it as a Windows .vst3 bundle.
#
#   ./bundle.sh <crate_name> <"Pretty Name"> [--deploy]
#
# --deploy also installs it into the Ableton Wine prefix's VST3 folder
# (override the prefix with WINEPREFIX).
set -euo pipefail

CRATE="${1:?usage: bundle.sh <crate> <pretty name> [--deploy]}"
NAME="${2:?usage: bundle.sh <crate> <pretty name> [--deploy]}"
TARGET="${TARGET:-x86_64-pc-windows-gnu}"

# Use the rustup toolchain (which has the windows-gnu std), not the system rust.
export PATH="$HOME/.cargo/bin:$PATH"

# mingw here ships no libgcc_eh.a; provide the stand-in (see .cargo/config.toml).
if [[ ! -f .cargo/mingw-shim/libgcc_eh.a ]]; then
  mkdir -p .cargo/mingw-shim
  cp "$(x86_64-w64-mingw32-gcc -print-libgcc-file-name)" .cargo/mingw-shim/libgcc_eh.a
fi

cargo build --release --target "$TARGET" -p "$CRATE"

BUNDLE="target/bundled/$NAME.vst3"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/x86_64-win"
cp "target/$TARGET/release/$CRATE.dll" "$BUNDLE/Contents/x86_64-win/$NAME.vst3"
echo ">> Bundled $BUNDLE"

if [[ "${3:-}" == "--deploy" ]]; then
  PREFIX="${WINEPREFIX:-$HOME/.wine-ableton}"
  DEST="$PREFIX/drive_c/Program Files/Common Files/VST3"
  if [[ ! -d "$PREFIX/drive_c" ]]; then
    echo "Wine prefix '$PREFIX' doesn't look right (no drive_c)." >&2
    exit 1
  fi
  mkdir -p "$DEST"
  rm -rf "$DEST/$NAME.vst3"
  cp -r "$BUNDLE" "$DEST/"
  echo ">> Installed $NAME.vst3 -> $DEST"
  echo ">> Rescan in Ableton (Preferences > Plug-Ins) or restart Live."
fi
