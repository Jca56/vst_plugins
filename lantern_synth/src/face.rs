//! The Blacklight face, rebuilt from a clean slate: controls move in one
//! round at a time, checked in the preview harness as they land.
//!
//! Current state: three sub-panel rooms across the top (OSC 1, OSC 2,
//! FILTER, no titles), output meter right, and a playable three-octave
//! keyboard (C0..C3) spanning the whole bottom — click to sound notes
//! (drag = glissando), keys light for whatever the voices are sounding,
//! MIDI included, out-of-range notes folding into view by octaves.

use lantern_ui::lantern_gpu::{Color, Rect};
use lantern_ui::{Theme, Ui, UiEditor};
use lantern_vst3::plugin::{Editor, ParamsHandle};

use crate::{
    p_osc, M_KEYS, M_LEVEL_L, M_LEVEL_R, M_PEAK_L, M_PEAK_R, M_RMS_L, M_RMS_R, M_UI_NOTE,
    OSC_DETUNE, OSC_ENABLE, OSC_PITCH, OSC_VOICES, OSC_VOLUME, OSC_WAVE,
};

const WIN_W: u32 = 1496;
const WIN_H: u32 = 884;

/// Keyboard range: C0 to C3 in Live's naming (Alva's home row on top).
const KB_LOW: u8 = 24;
const WHITE_KEYS: usize = 22; // 3 octaves + the top C

pub fn make_editor(params: ParamsHandle) -> Box<dyn Editor> {
    let mut face = Face::default();
    UiEditor::create(WIN_W, WIN_H, Theme::lantern(), params, move |ui| face.draw(ui))
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
    ui.painter.rect_filled(rect, 10.0, Color::from_rgb8(26, 26, 31));
    ui.painter.rect_border(rect, 10.0, 2.5, t.panel_border);
}

impl Face {
    fn draw(&mut self, ui: &mut Ui) {
        ui.face();

        // Three rooms across the top: OSC 1 | OSC 2 | FILTER (still empty).
        self.osc_panel(ui, Rect::new(44.0, 44.0, 406.0, 420.0), 0);
        self.osc_panel(ui, Rect::new(481.0, 44.0, 406.0, 420.0), 1);
        sub_panel(ui, Rect::new(918.0, 44.0, 406.0, 420.0));

        // Output meter ends above the keyboard.
        ui.level_meter(
            Rect::new(1372.0, 44.0, 80.0, 630.0),
            (M_LEVEL_L, M_LEVEL_R),
            (M_PEAK_L, M_PEAK_R),
            (M_RMS_L, M_RMS_R),
        );

        self.keyboard(ui, Rect::new(44.0, 690.0, 1408.0, 150.0));
    }

    /// One oscillator room: power square + wave dropdown across the top,
    /// then PITCH | VOLUME | (VOICES over DETUNE) left to right.
    fn osc_panel(&mut self, ui: &mut Ui, r: Rect, o: usize) {
        let t = ui.theme;
        sub_panel(ui, r);

        ui.toggle(p_osc(o, OSC_ENABLE), Rect::new(r.x + 12.0, r.y + 12.0, 36.0, 36.0), "");
        ui.dropdown(p_osc(o, OSC_WAVE), Rect::new(r.x + 58.0, r.y + 12.0, r.w - 70.0, 36.0));

        ui.knob_cell(p_osc(o, OSC_PITCH), Rect::new(r.x + 14.0, r.y + 64.0, 122.0, 190.0), "PITCH");
        ui.knob_cell(p_osc(o, OSC_VOLUME), Rect::new(r.x + 142.0, r.y + 64.0, 122.0, 190.0), "VOLUME");

        ui.label_centered("VOICES", r.x + 330.0, r.y + 64.0, 21.0, t.text_dim);
        ui.stepper_box(p_osc(o, OSC_VOICES), Rect::new(r.x + 294.0, r.y + 102.0, 72.0, 48.0));
        ui.knob_cell(p_osc(o, OSC_DETUNE), Rect::new(r.x + 268.0, r.y + 170.0, 124.0, 190.0), "DETUNE");
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
            ui.painter.rect_filled(r, 4.0, Color::from_rgb8(206, 206, 214));
            if lit(white_note(i)) {
                ui.painter.rect_filled(r, 4.0, t.cool.with_alpha(0.55));
            }
            if i % 7 == 0 {
                ui.label(
                    &format!("C{}", i / 7),
                    r.x + 7.0,
                    r.y + r.h - 28.0,
                    17.0,
                    Color::from_rgb8(45, 45, 55),
                );
            }
        }
        for oct in 0..3 {
            for &(pc, boundary) in &BLACKS {
                let r = black_rect(black_x(oct, boundary));
                ui.painter.rect_filled(r, 4.0, Color::from_rgb8(18, 18, 23));
                if lit(KB_LOW + oct as u8 * 12 + pc) {
                    ui.painter.rect_filled(r, 4.0, t.cool.with_alpha(0.65));
                }
                ui.painter.rect_border(r, 4.0, 2.5, t.panel_border);
            }
        }
    }
}
