//! The Lantern face: oscillators and filter across the top, the wobble
//! section and amp envelope mid, output and filter envelope below, meter
//! right. Wave selectors are choice rows; everything else is knob cells on
//! shared row lines.

use lantern_ui::lantern_gpu::Rect;
use lantern_ui::{Theme, Ui, UiEditor};
use lantern_vst3::plugin::{Editor, ParamsHandle};

use crate::{
    M_LEVEL_L, M_LEVEL_R, M_PEAK_L, M_PEAK_R, M_RMS_L, M_RMS_R, M_VOICES, P_AMP_A, P_AMP_D,
    P_AMP_R, P_AMP_S, P_CUTOFF, P_DRIVE, P_FENV_AMT, P_FLT_A, P_FLT_D, P_FLT_R, P_FLT_S, P_FM,
    P_GAIN, P_LFO_DEPTH, P_LFO_RATE, P_LFO_WAVE, P_O1_WAVE, P_O2_DETUNE, P_O2_OCTAVE, P_O2_WAVE,
    P_OSC_MIX, P_RES,
};

const WIN_W: u32 = 1360;
const WIN_H: u32 = 884;

const WAVE_NAMES: [&str; 4] = ["SIN", "TRI", "SAW", "SQR"];

pub fn make_editor(params: ParamsHandle) -> Box<dyn Editor> {
    UiEditor::create(WIN_W, WIN_H, Theme::lantern(), params, draw)
}

/// The face closure for the preview harness. Render at 1360x884.
pub fn preview_face() -> impl FnMut(&mut Ui) {
    draw
}

fn wave_row(ui: &mut Ui, param: usize, x: f32, y: f32) {
    for (i, name) in WAVE_NAMES.iter().enumerate() {
        ui.choice(param, i as i32, Rect::new(x + i as f32 * 88.0, y, 84.0, 48.0), name);
    }
}

fn draw(ui: &mut Ui) {
    ui.face();
    let t = ui.theme;

    // --- Row 1: OSC 1 | OSC 2 | FILTER (labels y44, waves y78, knobs y140)
    ui.label("OSC 1", 44.0, 44.0, 21.0, t.text_dim);
    wave_row(ui, P_O1_WAVE, 44.0, 78.0);
    ui.knob_cell(P_OSC_MIX, Rect::new(44.0, 140.0, 350.0, 232.0), "OSC MIX");

    ui.label("OSC 2", 434.0, 44.0, 21.0, t.text_dim);
    wave_row(ui, P_O2_WAVE, 434.0, 78.0);
    ui.knob_cell(P_O2_DETUNE, Rect::new(434.0, 140.0, 112.0, 232.0), "DETUNE");
    ui.knob_cell(P_O2_OCTAVE, Rect::new(551.0, 140.0, 112.0, 232.0), "OCTAVE");
    ui.knob_cell(P_FM, Rect::new(668.0, 140.0, 112.0, 232.0), "FM");

    ui.label("FILTER", 824.0, 44.0, 21.0, t.text_dim);
    ui.knob_cell(P_CUTOFF, Rect::new(824.0, 140.0, 124.0, 232.0), "CUTOFF");
    ui.knob_cell(P_RES, Rect::new(952.0, 140.0, 124.0, 232.0), "RES");
    ui.knob_cell(P_FENV_AMT, Rect::new(1080.0, 140.0, 124.0, 232.0), "ENV AMT");

    ui.divider(44.0, 396.0, 1160.0);

    // --- Row 2: WOBBLE | AMP ENV
    ui.label("WOBBLE", 44.0, 412.0, 21.0, t.text_dim);
    for (i, name) in WAVE_NAMES.iter().enumerate() {
        let (col, row) = (i % 2, i / 2);
        ui.choice(
            P_LFO_WAVE,
            i as i32,
            Rect::new(44.0 + col as f32 * 88.0, 462.0 + row as f32 * 54.0, 84.0, 48.0),
            name,
        );
    }
    ui.knob_cell(P_LFO_RATE, Rect::new(240.0, 438.0, 150.0, 194.0), "RATE");
    ui.knob_cell(P_LFO_DEPTH, Rect::new(404.0, 438.0, 150.0, 194.0), "DEPTH");

    ui.label("AMP ENV", 624.0, 412.0, 21.0, t.text_dim);
    for (i, (param, label)) in [
        (P_AMP_A, "ATTACK"),
        (P_AMP_D, "DECAY"),
        (P_AMP_S, "SUSTAIN"),
        (P_AMP_R, "RELEASE"),
    ]
    .iter()
    .enumerate()
    {
        ui.knob_cell(*param, Rect::new(624.0 + i as f32 * 117.0, 438.0, 112.0, 194.0), label);
    }

    // --- Row 3: OUT | voices | FILTER ENV
    ui.label("OUT", 44.0, 644.0, 21.0, t.text_dim);
    ui.knob_cell(P_DRIVE, Rect::new(44.0, 670.0, 112.0, 194.0), "DRIVE");
    ui.knob_cell(P_GAIN, Rect::new(161.0, 670.0, 112.0, 194.0), "GAIN");

    let voices = ui.params.meter(M_VOICES) as usize;
    ui.label_centered(
        &format!("{voices} VOICES"),
        448.0,
        756.0,
        21.0,
        if voices > 0 { t.accent } else { t.text_dim.with_alpha(0.5) },
    );

    ui.label("FILTER ENV", 624.0, 644.0, 21.0, t.text_dim);
    for (i, (param, label)) in [
        (P_FLT_A, "ATTACK"),
        (P_FLT_D, "DECAY"),
        (P_FLT_S, "SUSTAIN"),
        (P_FLT_R, "RELEASE"),
    ]
    .iter()
    .enumerate()
    {
        ui.knob_cell(*param, Rect::new(624.0 + i as f32 * 117.0, 670.0, 112.0, 194.0), label);
    }

    // Output meter, full height.
    ui.level_meter(
        Rect::new(1252.0, 44.0, 80.0, 820.0),
        (M_LEVEL_L, M_LEVEL_R),
        (M_PEAK_L, M_PEAK_R),
        (M_RMS_L, M_RMS_R),
    );
}
