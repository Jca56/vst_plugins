//! The Blacklight face, rebuilt from a clean slate: controls move in one
//! round at a time, checked in the preview harness as they land.
//!
//! Current state: three sub-panel rooms across the top (OSC 1, OSC 2,
//! FILTER, no titles), output meter right, and a playable mini keyboard
//! along the bottom — click to sound notes (drag = glissando), keys light
//! for whatever pitch classes the voices are sounding, MIDI included.

use lantern_ui::lantern_gpu::{Color, Rect};
use lantern_ui::{Theme, Ui, UiEditor};
use lantern_vst3::plugin::{Editor, ParamsHandle};

use crate::{M_KEYS, M_LEVEL_L, M_LEVEL_R, M_PEAK_L, M_PEAK_R, M_RMS_L, M_RMS_R, M_UI_NOTE};

const WIN_W: u32 = 1360;
const WIN_H: u32 = 884;

/// The mini keyboard's lowest note: C3 in Live's naming — where Alva lives.
const KB_ROOT: u8 = 60;

pub fn make_editor(params: ParamsHandle) -> Box<dyn Editor> {
    let mut face = Face::default();
    UiEditor::create(WIN_W, WIN_H, Theme::lantern(), params, move |ui| face.draw(ui))
}

/// The face closure for the preview harness. Render at 1360x884.
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

        // Three rooms across the top: OSC 1 | OSC 2 | FILTER, clear of the
        // meter on the right.
        for i in 0..3 {
            sub_panel(ui, Rect::new(44.0 + i as f32 * 396.0, 44.0, 366.0, 320.0));
        }

        self.keyboard(ui, Rect::new(368.0, 736.0, 512.0, 128.0));

        ui.level_meter(
            Rect::new(1252.0, 44.0, 80.0, 820.0),
            (M_LEVEL_L, M_LEVEL_R),
            (M_PEAK_L, M_PEAK_R),
            (M_RMS_L, M_RMS_R),
        );
    }

    /// One octave C-to-C, playable: mouse writes a gate the DSP edge-detects
    /// into real note on/offs; lighting comes back as a pitch-class mask, so
    /// MIDI-played notes glow the same as clicked ones.
    fn keyboard(&mut self, ui: &mut Ui, kb: Rect) {
        let t = ui.theme;
        // 8 white keys C..C; black keys sit on 5 of the 7 boundaries.
        const WHITE_PC: [u8; 8] = [0, 2, 4, 5, 7, 9, 11, 12];
        const BLACK_PC: [(u8, f32); 5] = [(1, 1.0), (3, 2.0), (6, 4.0), (8, 5.0), (10, 6.0)];
        let ww = kb.w / 8.0;
        let white_rect = |i: usize| Rect::new(kb.x + i as f32 * ww + 1.0, kb.y, ww - 2.0, kb.h);
        let black_rect =
            |bx: f32| Rect::new(kb.x + bx * ww - 18.0, kb.y, 36.0, kb.h * 0.62);

        // --- Interaction: press, glissando, release ---
        let (mx, my) = ui.mouse_pos();
        let mut over: Option<u8> = None;
        for &(pc, bx) in &BLACK_PC {
            if black_rect(bx).contains(mx, my) {
                over = Some(KB_ROOT + pc);
            }
        }
        if over.is_none() {
            for (i, &pc) in WHITE_PC.iter().enumerate() {
                if white_rect(i).contains(mx, my) {
                    over = Some(KB_ROOT + pc);
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

        // --- Drawing: lit = sounding (from the DSP's pitch-class mask) ---
        let mask = ui.params.meter(M_KEYS) as u32;
        let lit = |pc: u8| mask >> (pc % 12) & 1 == 1;

        for (i, &pc) in WHITE_PC.iter().enumerate() {
            let r = white_rect(i);
            ui.painter.rect_filled(r, 4.0, Color::from_rgb8(206, 206, 214));
            if lit(pc) {
                ui.painter.rect_filled(r, 4.0, t.accent.with_alpha(0.55));
            }
        }
        ui.label("C3", kb.x + 8.0, kb.y + kb.h - 28.0, 17.0, Color::from_rgb8(45, 45, 55));
        for &(pc, bx) in &BLACK_PC {
            let r = black_rect(bx);
            ui.painter.rect_filled(r, 4.0, Color::from_rgb8(18, 18, 23));
            if lit(pc) {
                ui.painter.rect_filled(r, 4.0, t.accent.with_alpha(0.65));
            }
            ui.painter.rect_border(r, 4.0, 2.5, t.panel_border);
        }
    }
}
