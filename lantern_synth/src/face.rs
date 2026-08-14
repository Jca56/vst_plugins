//! The Blacklight face, rebuilt from a clean slate: controls move in one
//! round at a time, checked in the preview harness as they land.
//!
//! Current state: three sub-panel rooms across the top (OSC 1, OSC 2,
//! FILTER, no titles), output meter right, and a playable three-octave
//! keyboard (C0..C3) spanning the whole bottom — click to sound notes
//! (drag = glissando), keys light for whatever the voices are sounding,
//! MIDI included, out-of-range notes folding into view by octaves.

use lantern_ui::lantern_gpu::{BackgroundImage, Color, Rect};
use lantern_ui::{Theme, Ui, UiEditor};
use lantern_vst3::plugin::{Editor, ParamsHandle};

use crate::synth::{osc, svf_k, FilterMode, Waveform};
use crate::{
    p_osc, M_KEYS, M_LEVEL_L, M_LEVEL_R, M_PEAK_L, M_PEAK_R, M_RMS_L, M_RMS_R, M_UI_NOTE,
    OSC_DETUNE, OSC_ENABLE, OSC_PITCH, OSC_VOICES, OSC_VOLUME, OSC_WAVE, P_CUTOFF, P_FENV_AMT,
    P_FLT_ON, P_FLT_TYPE, P_RES,
};

const WIN_W: u32 = 1776;
const WIN_H: u32 = 884;

/// Keyboard range: C0 to C3 in Live's naming (Alva's home row on top).
const KB_LOW: u8 = 24;
const WHITE_KEYS: usize = 22; // 3 octaves + the top C

/// The fire experiment: Alva's background image, embedded raw (888x442
/// RGBA), drawn under a translucent panel. If the verdict is "stupid",
/// this and `face_glass` revert to `face()` in one commit.
static BG_RGBA: &[u8] = include_bytes!("../assets/bg.rgba");

pub fn background() -> BackgroundImage {
    BackgroundImage {
        rgba: BG_RGBA.to_vec(),
        width: 888,
        height: 442,
        alpha: 0.38,
    }
}

pub fn make_editor(params: ParamsHandle) -> Box<dyn Editor> {
    let mut face = Face::default();
    UiEditor::create_with_background(
        WIN_W,
        WIN_H,
        Theme::lantern(),
        Some(background()),
        params,
        move |ui| face.draw(ui),
    )
}

/// The face closure for the preview harness. Render at 1496x884.
pub fn preview_face() -> impl FnMut(&mut Ui) {
    let mut face = Face::default();
    move |ui| face.draw(ui)
}

#[derive(Default)]
struct Face {
    /// Key currently held by the mouse (MIDI note).
    held: Option<u8>,
}

/// Sub-panel fill: one step lighter than the face panel.
fn sub_panel(ui: &mut Ui, rect: Rect) {
    let t = ui.theme;
    ui.painter.rect_filled(rect, 10.0, Color::from_rgb8(27, 27, 27));
    ui.painter.rect_border(rect, 10.0, 2.5, t.panel_border);
}

/// The filter room: power + type dropdown on top, the live response curve
/// beneath (drawn from the same SVF math the audio runs), then
/// CUTOFF | RES | ENV AMT with a spare slot for what comes later.
fn filter_panel(ui: &mut Ui, r: Rect) {
    sub_panel(ui, r);

    ui.toggle(P_FLT_ON, Rect::new(r.x + 12.0, r.y + 14.0, 48.0, 48.0), "");
    ui.dropdown(P_FLT_TYPE, Rect::new(r.x + 68.0, r.y + 14.0, r.w - 82.0, 48.0));

    response_window(ui, Rect::new(r.x + 16.0, r.y + 74.0, r.w - 32.0, 130.0));

    // Three knobs, centered in the row.
    let start = r.x + (r.w - 350.0) * 0.5;
    for (i, (param, label)) in [(P_CUTOFF, "CUTOFF"), (P_RES, "RES"), (P_FENV_AMT, "ENV AMT")]
        .iter()
        .enumerate()
    {
        ui.knob_cell(
            *param,
            Rect::new(start + i as f32 * 118.0, r.y + 216.0, 114.0, 190.0),
            label,
        );
    }
}

/// The filter's magnitude response, live from the current cutoff, res,
/// and type — what you hear is what it draws. Log frequency 20..20k,
/// +/-24 dB.
fn response_window(ui: &mut Ui, rect: Rect) {
    let t = ui.theme;
    ui.painter.rect_filled(rect, 8.0, t.well_deep);

    let fc = ui.params.plain(P_CUTOFF) as f32;
    let res = ui.params.plain(P_RES) as f32 / 100.0;
    let k = svf_k(res);
    let mode = FilterMode::from_index((ui.params.normalized(P_FLT_TYPE) * 3.0).round() as usize);
    let on = ui.params.normalized(P_FLT_ON) >= 0.5;

    let y0 = rect.center_y();
    ui.painter.line(rect.x + 6.0, y0, rect.x + rect.w - 6.0, y0, 1.0, t.track);

    let pad = 10.0;
    let n = 128;
    let mut prev: Option<(f32, f32)> = None;
    let color = if on { t.accent } else { t.text_dim.with_alpha(0.4) };
    ui.painter.push_clip(rect.inset(3.0));
    for i in 0..=n {
        let tx = i as f32 / n as f32;
        let f = 20.0 * 1000f32.powf(tx);
        let x_rel = f / fc.max(1.0);
        let d = ((1.0 - x_rel * x_rel).powi(2) + (k * x_rel).powi(2)).sqrt().max(1e-6);
        let mag = match mode {
            FilterMode::Lowpass => 1.0 / d,
            FilterMode::Bandpass => k * x_rel / d,
            FilterMode::Highpass => x_rel * x_rel / d,
            FilterMode::Notch => (1.0 - x_rel * x_rel).abs() / d,
        };
        let db = (20.0 * mag.max(1e-4).log10()).clamp(-40.0, 40.0);
        let x = rect.x + pad + tx * (rect.w - pad * 2.0);
        let y = y0 - db / 24.0 * rect.h * 0.42;
        if let Some((px, py)) = prev {
            ui.painter.line(px, py, x, y, 2.5, color);
        }
        prev = Some((x, y));
    }
    ui.painter.pop_clip();
    ui.painter.rect_border(rect, 8.0, 2.5, t.panel_border);
}

/// The shape window: today a still of the selected waveform, one day the
/// live oscilloscope.
fn wave_window(ui: &mut Ui, rect: Rect, wave: Waveform) {
    let t = ui.theme;
    ui.painter.rect_filled(rect, 8.0, t.well_deep);
    // Zero line.
    ui.painter.line(
        rect.x + 6.0,
        rect.center_y(),
        rect.x + rect.w - 6.0,
        rect.center_y(),
        1.0,
        t.track,
    );

    let n = 128;
    let pad = 14.0;
    let (w, half) = (rect.w - pad * 2.0, rect.h * 0.36);
    let mut rng = 0x00C0_FFEEu32;
    let mut prev: Option<(f32, f32)> = None;
    ui.painter.push_clip(rect.inset(3.0));
    for i in 0..=n {
        let phase = i as f32 / n as f32;
        let s = osc(wave, phase % 1.0, 1.0 / n as f32, &mut rng).clamp(-1.2, 1.2);
        let x = rect.x + pad + phase * w;
        let y = rect.center_y() - s * half;
        if let Some((px, py)) = prev {
            ui.painter.line(px, py, x, y, 2.5, t.accent);
        }
        prev = Some((x, y));
    }
    ui.painter.pop_clip();
    ui.painter.rect_border(rect, 8.0, 2.5, t.panel_border);
}

impl Face {
    fn draw(&mut self, ui: &mut Ui) {
        ui.face_glass(0.72);

        // Three wide rooms across the top: OSC 1 | OSC 2 | FILTER.
        self.osc_panel(ui, Rect::new(44.0, 44.0, 500.0, 420.0), 0);
        self.osc_panel(ui, Rect::new(574.0, 44.0, 500.0, 420.0), 1);
        filter_panel(ui, Rect::new(1104.0, 44.0, 500.0, 420.0));

        // Output meter ends above the keyboard.
        ui.level_meter(
            Rect::new(1652.0, 44.0, 80.0, 630.0),
            (M_LEVEL_L, M_LEVEL_R),
            (M_PEAK_L, M_PEAK_R),
            (M_RMS_L, M_RMS_R),
        );

        self.keyboard(ui, Rect::new(44.0, 690.0, 1688.0, 150.0));
    }

    /// One oscillator room, wide format: power square + wave dropdown on
    /// top, the wave window (oscilloscope-to-be) beneath, then four equal
    /// knobs: VOICES | DETUNE | PITCH | VOLUME.
    fn osc_panel(&mut self, ui: &mut Ui, r: Rect, o: usize) {
        sub_panel(ui, r);

        ui.toggle(p_osc(o, OSC_ENABLE), Rect::new(r.x + 12.0, r.y + 14.0, 48.0, 48.0), "");
        ui.dropdown(p_osc(o, OSC_WAVE), Rect::new(r.x + 68.0, r.y + 14.0, r.w - 82.0, 48.0));

        let wave = Waveform::from_index(
            (ui.params.normalized(p_osc(o, OSC_WAVE)) * 7.0).round() as usize,
        );
        wave_window(ui, Rect::new(r.x + 16.0, r.y + 74.0, r.w - 32.0, 130.0), wave);

        for (i, (which, label)) in [
            (OSC_VOICES, "VOICES"),
            (OSC_DETUNE, "DETUNE"),
            (OSC_PITCH, "PITCH"),
            (OSC_VOLUME, "VOLUME"),
        ]
        .iter()
        .enumerate()
        {
            ui.knob_cell(
                p_osc(o, *which),
                Rect::new(r.x + 16.0 + i as f32 * 118.0, r.y + 216.0, 114.0, 190.0),
                label,
            );
        }
    }

    /// C0..C3, playable: the mouse writes a gate the DSP edge-detects into
    /// real note on/offs; lighting comes back as a per-key mask, so
    /// MIDI-played notes glow the same as clicked ones.
    fn keyboard(&mut self, ui: &mut Ui, kb: Rect) {
        let t = ui.theme;
        const DEG_PC: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
        // Black keys per octave: (pitch-class offset, white-boundary index).
        const BLACKS: [(u8, usize); 5] = [(1, 1), (3, 2), (6, 4), (8, 5), (10, 6)];
        let ww = kb.w / WHITE_KEYS as f32;
        let bw = (ww * 0.58).min(40.0);

        let white_note = |i: usize| KB_LOW + (i / 7) as u8 * 12 + DEG_PC[i % 7];
        let white_rect = |i: usize| Rect::new(kb.x + i as f32 * ww + 1.0, kb.y, ww - 2.0, kb.h);
        let black_x = |oct: usize, boundary: usize| kb.x + (oct * 7 + boundary) as f32 * ww;
        let black_rect =
            |cx: f32| Rect::new(cx - bw * 0.5, kb.y, bw, kb.h * 0.62);

        // --- Interaction: press, glissando, release ---
        let (mx, my) = ui.mouse_pos();
        let mut over: Option<u8> = None;
        'blacks: for oct in 0..3 {
            for &(pc, boundary) in &BLACKS {
                if black_rect(black_x(oct, boundary)).contains(mx, my) {
                    over = Some(KB_LOW + oct as u8 * 12 + pc);
                    break 'blacks;
                }
            }
        }
        if over.is_none() {
            for i in 0..WHITE_KEYS {
                if white_rect(i).contains(mx, my) {
                    over = Some(white_note(i));
                    break;
                }
            }
        }
        if ui.mouse_clicked() && over.is_some() {
            self.held = over;
            ui.params.meter_set(M_UI_NOTE, (over.unwrap() as f64) + 1.0);
        } else if ui.mouse_is_down() && self.held.is_some() {
            if let Some(n) = over {
                if Some(n) != self.held {
                    self.held = Some(n);
                    ui.params.meter_set(M_UI_NOTE, n as f64 + 1.0);
                }
            }
        } else if self.held.is_some() && !ui.mouse_is_down() {
            self.held = None;
            ui.params.meter_set(M_UI_NOTE, 0.0);
        }

        // --- Drawing: lit = sounding (per-key mask from the DSP) ---
        let mask = ui.params.meter(M_KEYS) as u64;
        let lit = |note: u8| mask >> (note - KB_LOW) & 1 == 1;

        for i in 0..WHITE_KEYS {
            let r = white_rect(i);
            ui.painter.rect_filled(r, 4.0, Color::from_rgb8(208, 208, 208));
            if lit(white_note(i)) {
                ui.painter.rect_filled(r, 4.0, t.cool.with_alpha(0.55));
            }
            if i % 7 == 0 {
                ui.label(
                    &format!("C{}", i / 7),
                    r.x + 7.0,
                    r.y + r.h - 28.0,
                    17.0,
                    Color::from_rgb8(48, 48, 48),
                );
            }
        }
        for oct in 0..3 {
            for &(pc, boundary) in &BLACKS {
                let r = black_rect(black_x(oct, boundary));
                ui.painter.rect_filled(r, 4.0, Color::from_rgb8(20, 20, 20));
                if lit(KB_LOW + oct as u8 * 12 + pc) {
                    ui.painter.rect_filled(r, 4.0, t.cool.with_alpha(0.65));
                }
                ui.painter.rect_border(r, 4.0, 2.5, t.panel_border);
            }
        }
    }
}
