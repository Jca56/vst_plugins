#!/usr/bin/env bash
# Cross-compile Lantern into a Windows VST3 (+ CLAP) bundle, so Ableton Live
# running under Wine can load it.
set -euo pipefail

# Use the rustup toolchain (which has the windows-gnu std), not the system rust.
export PATH="$HOME/.cargo/bin:$PATH"

TARGET="${TARGET:-x86_64-pc-windows-gnu}"

# This mingw folds exception-handling into libgcc.a and ships no separate
# libgcc_eh.a, which Rust's windows-gnu target asks for. Provide a stand-in so
# `-lgcc_eh` resolves (the real EH symbols come from libgcc.a, also linked).
SHIM_DIR=".cargo/mingw-shim"
if [[ ! -f "$SHIM_DIR/libgcc_eh.a" ]]; then
  mkdir -p "$SHIM_DIR"
  LIBGCC="$(x86_64-w64-mingw32-gcc -print-libgcc-file-name)"
  cp "$LIBGCC" "$SHIM_DIR/libgcc_eh.a"
  echo ">> Created libgcc_eh.a shim from $LIBGCC"
fi

echo ">> Building Lantern for $TARGET (release)..."
cargo xtask bundle lantern_synth --release --target "$TARGET"

echo
echo ">> Done. Bundles in target/bundled/:"
ls -d target/bundled/*.vst3 target/bundled/*.clap 2>/dev/null || true
