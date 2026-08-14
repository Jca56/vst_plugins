#!/usr/bin/env bash
# Fast UI preview: build & run Lantern as a NATIVE Linux standalone app.
# No cross-compile, no Wine, no Ableton — opens the VIZIA editor in a window
# with live audio. Edit src/editor.rs, re-run, and incremental builds are seconds.
# Close the window or Ctrl-C to quit.  Add --release for optimized audio.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
exec cargo run --features standalone --bin lantern_standalone "$@"
