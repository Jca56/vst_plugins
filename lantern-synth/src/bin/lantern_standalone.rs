//! Native standalone build of Lantern — opens the GUI in a window with live
//! audio (ALSA/JACK), for fast UI iteration without Wine/Ableton.
//! Run via `./gui.sh` (or `cargo run --features standalone --bin lantern_standalone`).

use nih_plug::prelude::nih_export_standalone;

fn main() {
    nih_export_standalone::<lantern_synth::LanternSynth>();
}
