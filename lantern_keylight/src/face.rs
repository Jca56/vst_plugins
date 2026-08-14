//! The Keylight face: tuner, pitch, and utility down the left; the scale
//! cheat sheet — root row, scale grid, lit piano strip, and nearest-note
//! hints — on the right. Clicking a piano key retunes SHIFT so the detected
//! note lands on it.

use lantern_ui::lantern_gpu::{Color, Rect};
use lantern_ui::{Theme, Ui, UiEditor};
use lantern_vst3::plugin::{Editor, ParamsHandle};

use crate::music::{degree_name, Scale, NOTE_NAMES, SCALES};
use crate::{
    M_CLARITY, M_FREQ, M_LEVEL_L, M_LEVEL_R, M_PEAK_L, M_PEAK_R, M_RMS_L, M_RMS_R, P_BALANCE,
    P_BASS_FREQ, P_BASS_MONO, P_FINE, P_GAIN, P_MONO, P_ROOT, P_SCALE, P_SHIFT,
};

/// Collapsed = controls + meter; expanded = with the scale guide. The
/// output meter rides the window's right edge when collapsed and reads as
/// the divider between controls and cheat sheet when expanded.
const COLLAPSED_W: u32 = 662;
const EXPANDED_W: u32 = 1300;
const WIN_H: u32 = 800;

pub fn make_editor(params: ParamsHandle) -> Box<dyn Editor> {
    let mut face = Face::default();
    let width = if load_expanded() { EXPANDED_W } else { COLLAPSED_W };
    UiEditor::create(width, WIN_H, Theme::lantern(), params, move |ui| face.draw(ui))
}

/// The face closure for the preview harness (no host, no window). Render
/// at 1300x800 for the expanded face, 662x800 for collapsed.
pub fn preview_face() -> impl FnMut(&mut Ui) {
    let mut face = Face::default();
    move |ui| face.draw(ui)
}

// The expand/collapse choice is a UI preference, not project data: it lives
// in a one-word config file, so every Keylight opens the way the last one
// was left.

fn cfg_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(std::path::PathBuf::from(base).join("Lantern").join("keylight_ui.cfg"))
}

fn load_expanded() -> bool {
    cfg_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|s| s.trim() == "expanded")
}

fn save_expanded(expanded: bool) {
    if let Some(path) = cfg_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, if expanded { "expanded" } else { "collapsed" });
    }
}

/// The note the tuner last locked onto (held on screen after the signal
/// stops, dimmed).
#[derive(Clone, Copy)]
struct Held {
    midi: i32,
    cents: f32,
    freq: f32,
}

#[derive(Default)]
struct Face {
    held: Option<Held>,
    /// Tuner display brightness, fading toward dim while nothing plays.
    bright: f32,
}

const LEFT_X: f32 = 40.0;
const LEFT_W: f32 = 470.0;
const RIGHT_X: f32 = 660.0;
const RIGHT_W: f32 = 600.0;

impl Face {
    fn draw(&mut self, ui: &mut Ui) {
        ui.face();

        // Fold the live meters into the held note before anything draws.
        let freq = ui.params.meter(M_FREQ) as f32;
        let clarity = ui.params.meter(M_CLARITY) as f32;
        let live = clarity > 0.5 && freq > 16.0;
        if live {
            let midi_f = 69.0 + 12.0 * (freq / 440.0).log2();
            let midi = midi_f.round() as i32;
            self.held = Some(Held {
                midi,
                cents: (midi_f - midi as f32) * 100.0,
                freq,
            });
        }
        self.bright += ((if live { 1.0 } else { 0.35 }) - self.bright) * 0.12;

        // Left column: tuner, pitch, utility.
        self.tuner(ui, Rect::new(LEFT_X, 44.0, LEFT_W, 250.0));
        ui.knob(P_SHIFT, 165.0, 430.0, 64.0, "SHIFT");
        // Smaller knob, but framed at SHIFT's radius so label and value box
        // sit on the same lines as its big sibling.
        ui.knob_framed(P_FINE, 385.0, 430.0, 54.0, 64.0, "FINE");
        self.snap_button(ui);
        ui.divider(LEFT_X, 560.0, LEFT_W);
        // Wider stance than the SHIFT/FINE pair above, the mono stack on
        // the center line: the column reads as a broad-based pyramid.
        ui.knob(P_BALANCE, 110.0, 668.0, 54.0, "BALANCE");
        ui.toggle(P_MONO, Rect::new(213.0, 594.0, 124.0, 48.0), "MONO");
        ui.toggle(P_BASS_MONO, Rect::new(213.0, 650.0, 124.0, 48.0), "BASS MONO");
        ui.drag_box(P_BASS_FREQ, Rect::new(219.0, 706.0, 112.0, 34.0));
        ui.knob(P_GAIN, 440.0, 668.0, 54.0, "GAIN");

        // Output meter along the window's right edge (collapsed) / between
        // the columns (expanded).
        ui.level_meter(
            Rect::new(526.0, 44.0, 80.0, 720.0),
            (M_LEVEL_L, M_LEVEL_R),
            (M_PEAK_L, M_PEAK_R),
            (M_RMS_L, M_RMS_R),
        );

        // Right column: the cheat sheet, when the host granted us the width
        // for it. Drawing keys off the real window size, not intent, so a
        // refused resize can't leave phantom widgets.
        let wide = ui.width >= EXPANDED_W as f32 - 1.0;
        self.expand_tab(ui, wide);
        if wide {
            self.scale_guide(ui);
        }
    }

    /// The tab on the left column's right edge that folds the scale guide
    /// in and out. Sits in the gap between the columns, so it never moves.
    fn expand_tab(&mut self, ui: &mut Ui, wide: bool) {
        let t = ui.theme;
        let tab = Rect::new(614.0, 362.0, 32.0, 76.0);
        let (mx, my) = ui.mouse_pos();
        let hovered = tab.contains(mx, my);
        if hovered && ui.mouse_clicked() {
            let target = !wide;
            save_expanded(target);
            ui.request_size(if target { EXPANDED_W } else { COLLAPSED_W }, WIN_H);
        }

        ui.painter
            .rect_filled(tab, 6.0, if hovered { t.track } else { t.well });
        ui.painter.rect_border(
            tab,
            6.0,
            2.5,
            if hovered { t.text_dim } else { t.panel_border },
        );
        let glyph = if wide { "«" } else { "»" };
        let color = if hovered { t.text } else { t.text_dim };
        ui.label_centered(glyph, tab.center_x(), tab.center_y() - 16.0, 26.0, color);
    }

    /// One click to land the detected note on the nearest in-scale pitch —
    /// the shortcut past even reading the suggestion line. Ties go down,
    /// because darker.
    fn snap_button(&mut self, ui: &mut Ui) {
        let enabled = self.held.is_some();
        if ui.button(Rect::new(233.0, 498.0, 84.0, 46.0), "SNAP", enabled) {
            let Some(h) = self.held else { return };
            let root = (ui.params.plain(P_ROOT) as usize) % 12;
            let scale = &SCALES[(ui.params.plain(P_SCALE) as usize).min(SCALES.len() - 1)];
            let pc = h.midi.rem_euclid(12) as usize;
            let rel = (pc + 12 - root) % 12;
            if scale.contains(rel as u8) {
                return; // already home
            }
            let down = (1..=6).find(|k| scale.contains(((rel + 12 - k) % 12) as u8)).unwrap_or(0);
            let up = (1..=6).find(|k| scale.contains(((rel + k) % 12) as u8)).unwrap_or(0);
            let delta = if down <= up { -(down as f64) } else { up as f64 };
            let shift = (ui.params.plain(P_SHIFT) + delta).clamp(-12.0, 12.0);
            ui.params.begin_edit(P_SHIFT);
            ui.params.perform_edit(P_SHIFT, (shift + 12.0) / 24.0);
            ui.params.end_edit(P_SHIFT);
        }
    }

    // ------------------------------------------------------------------
    // Tuner well
    // ------------------------------------------------------------------

    fn tuner(&self, ui: &mut Ui, rect: Rect) {
        let t = ui.theme;
        let b = self.bright;

        ui.painter.rect_filled(rect, 12.0, t.well_deep);
        ui.painter
            .inner_shadow(rect, 12.0, 10.0, Color::rgba(0.0, 0.0, 0.0, 0.5), 0.0, 4.0);
        ui.painter.rect_border(rect, 12.0, 3.0, t.panel_border);

        let (note_txt, cents, detail) = match self.held {
            Some(h) => (
                format!(
                    "{}{}",
                    NOTE_NAMES[h.midi.rem_euclid(12) as usize],
                    h.midi.div_euclid(12) - 1
                ),
                Some(h.cents),
                format!("{:.1} Hz  ·  {:+.0} ct", h.freq, h.cents),
            ),
            None => ("—".to_string(), None, "listening…".to_string()),
        };
        ui.label_centered(&note_txt, rect.center_x(), rect.y + 30.0, 92.0, t.text.with_alpha(b));

        // Cents bar: -50 .. +50, needle turns accent within +/-7 ct.
        let bar = Rect::new(rect.x + 60.0, rect.y + 168.0, rect.w - 120.0, 8.0);
        ui.painter.rect_filled(bar, 3.0, t.track);
        ui.painter.line(
            bar.center_x(),
            bar.y - 8.0,
            bar.center_x(),
            bar.y + bar.h + 8.0,
            2.0,
            t.text_dim,
        );
        ui.label("-50", rect.x + 16.0, bar.y - 8.0, 18.0, t.text_dim);
        ui.label("+50", bar.x + bar.w + 12.0, bar.y - 8.0, 18.0, t.text_dim);
        if let Some(c) = cents {
            let x = bar.center_x() + (c / 50.0).clamp(-1.0, 1.0) * bar.w * 0.5;
            let in_tune = c.abs() <= 7.0;
            let col = if in_tune { t.accent } else { t.warm };
            if in_tune && b > 0.6 {
                ui.painter.rect_radial_glow(
                    Rect::new(x - 26.0, bar.center_y() - 26.0, 52.0, 52.0),
                    26.0,
                    (0.5, 0.5),
                    26.0,
                    col.with_alpha(0.25 * b),
                );
            }
            ui.painter.circle_filled(x, bar.center_y(), 9.0, col.with_alpha(b));
        }

        ui.label_centered(&detail, rect.center_x(), rect.y + 200.0, 22.0, t.text_dim.with_alpha(b));
    }

    // ------------------------------------------------------------------
    // Scale guide
    // ------------------------------------------------------------------

    fn scale_guide(&mut self, ui: &mut Ui) {
        let t = ui.theme;

        ui.label("ROOT", RIGHT_X, 38.0, 20.0, t.text_dim);
        for i in 0..12usize {
            ui.choice(
                P_ROOT,
                i as i32,
                Rect::new(RIGHT_X + i as f32 * 50.0, 66.0, 46.0, 52.0),
                NOTE_NAMES[i],
            );
        }

        ui.label("SCALE", RIGHT_X, 136.0, 20.0, t.text_dim);
        for (i, s) in SCALES.iter().enumerate() {
            let col = (i / 6) as f32;
            let row = (i % 6) as f32;
            ui.choice(
                P_SCALE,
                i as i32,
                Rect::new(RIGHT_X + col * 304.0, 164.0 + row * 56.0, 296.0, 48.0),
                s.name,
            );
        }

        let root = (ui.params.plain(P_ROOT) as usize) % 12;
        let scale = &SCALES[(ui.params.plain(P_SCALE) as usize).min(SCALES.len() - 1)];
        self.piano(ui, Rect::new(RIGHT_X, 530.0, RIGHT_W, 170.0), root, scale);
        self.suggestion(ui, RIGHT_X + RIGHT_W * 0.5, 712.0, root, scale);
    }

    fn piano(&self, ui: &mut Ui, kb: Rect, root: usize, scale: &Scale) {
        let t = ui.theme;
        let in_scale = |pc: usize| scale.contains(((pc + 12 - root) % 12) as u8);
        let det_pc = self.held.map(|h| h.midi.rem_euclid(12) as usize);

        const WHITE_PC: [usize; 7] = [0, 2, 4, 5, 7, 9, 11];
        // (pitch class, position in white-key widths from the left edge)
        const BLACK_PC: [(usize, f32); 5] = [(1, 1.0), (3, 2.0), (6, 4.0), (8, 5.0), (10, 6.0)];
        let ww = kb.w / 7.0;
        let white_rect = |i: usize| Rect::new(kb.x + i as f32 * ww + 1.0, kb.y, ww - 2.0, kb.h);
        let black_rect = |bx: f32| Rect::new(kb.x + bx * ww - 24.0, kb.y, 48.0, 104.0);

        // Click-to-tune: nudge SHIFT so the detected note lands on the
        // clicked key (black keys sit on top, so they hit-test first).
        let (mx, my) = ui.mouse_pos();
        if ui.mouse_clicked() && kb.contains(mx, my) {
            let mut target = None;
            for &(pc, bx) in &BLACK_PC {
                if black_rect(bx).contains(mx, my) {
                    target = Some(pc);
                }
            }
            if target.is_none() {
                for (i, &pc) in WHITE_PC.iter().enumerate() {
                    if white_rect(i).contains(mx, my) {
                        target = Some(pc);
                    }
                }
            }
            if let (Some(tpc), Some(dpc)) = (target, det_pc) {
                let delta = (tpc as i32 - dpc as i32 + 6).rem_euclid(12) - 6;
                let shift = (ui.params.plain(P_SHIFT) + delta as f64).clamp(-12.0, 12.0);
                ui.params.begin_edit(P_SHIFT);
                ui.params.perform_edit(P_SHIFT, (shift + 12.0) / 24.0);
                ui.params.end_edit(P_SHIFT);
            }
        }

        for (i, &pc) in WHITE_PC.iter().enumerate() {
            let r = white_rect(i);
            ui.painter.rect_filled(r, 4.0, Color::from_rgb8(208, 208, 208));
            if in_scale(pc) {
                let a = if pc == root { 0.72 } else { 0.38 };
                ui.painter.rect_filled(r, 4.0, t.cool.with_alpha(a));
            }
            ui.label_centered(
                NOTE_NAMES[pc],
                r.center_x(),
                r.y + r.h - 34.0,
                19.0,
                Color::from_rgb8(48, 48, 48),
            );
        }
        for &(pc, bx) in &BLACK_PC {
            let r = black_rect(bx);
            ui.painter.rect_filled(r, 4.0, Color::from_rgb8(20, 20, 20));
            if in_scale(pc) {
                let a = if pc == root { 0.85 } else { 0.55 };
                ui.painter.rect_filled(r, 4.0, t.cool.with_alpha(a));
            }
            ui.painter.rect_border(r, 4.0, 2.5, t.panel_border);
        }

        // Detected-note marker, pulsing: accent in scale, warm outside.
        if let Some(pc) = det_pc {
            let (cx, cy) = match BLACK_PC.iter().find(|&&(b, _)| b == pc) {
                Some(&(_, bx)) => (kb.x + bx * ww, kb.y + 82.0),
                None => {
                    let i = WHITE_PC.iter().position(|&w| w == pc).unwrap_or(0);
                    (white_rect(i).center_x(), kb.y + kb.h - 60.0)
                }
            };
            let col = if in_scale(pc) { t.accent } else { t.warm };
            let pulse = self.bright * (0.7 + 0.3 * (ui.time * 5.0).sin());
            ui.painter.rect_radial_glow(
                Rect::new(cx - 36.0, cy - 36.0, 72.0, 72.0),
                36.0,
                (0.5, 0.5),
                36.0,
                col.with_alpha(0.35 * pulse),
            );
            ui.painter
                .circle_filled(cx, cy, 9.0, col.with_alpha(0.4 + 0.6 * pulse));
        }
    }

    fn suggestion(&self, ui: &mut Ui, cx: f32, y: f32, root: usize, scale: &Scale) {
        let t = ui.theme;
        let b = 0.15 + 0.85 * self.bright;
        let Some(h) = self.held else {
            ui.label_centered("play something — listening…", cx, y, 24.0, t.text_dim.with_alpha(0.6));
            return;
        };
        let pc = h.midi.rem_euclid(12) as usize;
        let name = format!("{}{}", NOTE_NAMES[pc], h.midi.div_euclid(12) - 1);
        let rel = (pc + 12 - root) % 12;
        if scale.contains(rel as u8) {
            let txt = format!(
                "{} ✓  {} of {} {}",
                name,
                degree_name(rel),
                NOTE_NAMES[root],
                scale.name
            );
            ui.label_centered(&txt, cx, y, 24.0, t.accent.with_alpha(b));
        } else {
            let down = (1..=6).find(|k| scale.contains(((rel + 12 - k) % 12) as u8)).unwrap_or(0);
            let up = (1..=6).find(|k| scale.contains(((rel + k) % 12) as u8)).unwrap_or(0);
            let txt = format!(
                "{} is outside {} {}  →  {} -{} st  ·  {} +{} st",
                name,
                NOTE_NAMES[root],
                scale.name,
                NOTE_NAMES[(pc + 12 - down) % 12],
                down,
                NOTE_NAMES[(pc + up) % 12],
                up
            );
            ui.label_centered(&txt, cx, y, 24.0, t.warm.with_alpha(b));
        }
    }
}
