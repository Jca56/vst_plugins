//! The EQ face: selected-band knobs on the left, the graph — spectrum,
//! band curves, combined curve, draggable handles — front and center,
//! per-band enable row with type switches below, output meter right.
//!
//! Interaction: drag a handle for freq/gain, wheel over the graph for the
//! selected band's Q, click a handle or band button to select. The left
//! knobs (with typed entry) edit the selected band precisely.

use lantern_ui::lantern_gpu::{Color, Rect};
use lantern_ui::{Theme, Ui, UiEditor};
use lantern_vst3::plugin::{Editor, ParamsHandle};

use crate::biquad::{self, Mode};
use crate::{
    p_enabled, p_freq, p_gain, p_q, BANDS, M_LEVEL_L, M_LEVEL_R, M_PEAK_L, M_PEAK_R, M_RMS_L,
    M_RMS_R, M_SAMPLE_RATE, M_SPECTRUM, P_TYPE_HI, P_TYPE_LO, SPEC_BINS,
};

const WIN_W: u32 = 1340;
const WIN_H: u32 = 820;

const GRAPH: Rect = Rect {
    x: 220.0,
    y: 44.0,
    w: 960.0,
    h: 630.0,
};
/// Curve display range in dB (gain range is ±15).
const CURVE_DB: f32 = 18.0;

pub fn make_editor(params: ParamsHandle) -> Box<dyn Editor> {
    let mut face = Face::new();
    UiEditor::create(WIN_W, WIN_H, Theme::lantern(), params, move |ui| face.draw(ui))
}

/// The face closure for the preview harness (no host, no window).
/// Render at 1340x820.
pub fn preview_face() -> impl FnMut(&mut Ui) {
    let mut face = Face::new();
    move |ui| face.draw(ui)
}

fn band_color(band: usize) -> Color {
    match band {
        0 => Color::from_rgb8(235, 74, 62),   // red
        1 => Color::from_rgb8(74, 205, 110),  // green
        2 => Color::from_rgb8(84, 140, 250),  // blue
        3 => Color::from_rgb8(255, 178, 32),  // gold
        _ => Color::from_rgb8(186, 108, 245), // purple
    }
}

struct Face {
    selected: usize,
    /// Band currently dragged by its graph handle.
    dragging: Option<usize>,
    /// Smoothed spectrum (fast rise, slow fall), dB.
    spec: [f32; SPEC_BINS],
}

impl Face {
    fn new() -> Self {
        Self {
            selected: 1,
            dragging: None,
            spec: [-90.0; SPEC_BINS],
        }
    }

    fn sample_rate(&self, ui: &Ui) -> f32 {
        let sr = ui.params.meter(M_SAMPLE_RATE) as f32;
        if sr > 1000.0 {
            sr
        } else {
            48_000.0
        }
    }

    fn mode(&self, ui: &Ui, band: usize) -> Mode {
        match band {
            0 => {
                if ui.params.normalized(P_TYPE_LO) < 0.5 {
                    Mode::LowCut
                } else {
                    Mode::LowShelf
                }
            }
            4 => {
                if ui.params.normalized(P_TYPE_HI) < 0.5 {
                    Mode::HighCut
                } else {
                    Mode::HighShelf
                }
            }
            _ => Mode::Bell,
        }
    }

    /// Cut bands pin their handle (and curve identity) to 0 dB.
    fn has_gain(&self, ui: &Ui, band: usize) -> bool {
        !matches!(self.mode(ui, band), Mode::LowCut | Mode::HighCut)
    }

    fn draw(&mut self, ui: &mut Ui) {
        ui.face();
        let t = ui.theme;

        // Left column: selected-band header + Freq/Gain/Q for that band.
        let sel = self.selected;
        let mode_name = match self.mode(ui, sel) {
            Mode::LowCut => "HPF",
            Mode::LowShelf => "LOW SHELF",
            Mode::Bell => "BELL",
            Mode::HighShelf => "HIGH SHELF",
            Mode::HighCut => "LPF",
        };
        ui.label(
            &format!("BAND {} · {}", sel + 1, mode_name),
            44.0,
            GRAPH.y,
            21.0,
            band_color(sel),
        );
        let left = Rect::new(44.0, 80.0, 140.0, 602.0);
        ui.knob_cell(p_freq(sel), left.row(0, 3, 16.0), "FREQ");
        ui.knob_cell(p_gain(sel), left.row(1, 3, 16.0), "GAIN");
        ui.knob_cell(p_q(sel), left.row(2, 3, 16.0), "Q");

        self.graph(ui);
        self.band_row(ui);

        // Output meter, graph top line to below the band row.
        ui.level_meter(
            Rect::new(1220.0, GRAPH.y, 80.0, 746.0),
            (M_LEVEL_L, M_LEVEL_R),
            (M_PEAK_L, M_PEAK_R),
            (M_RMS_L, M_RMS_R),
        );
        let _ = t;
    }

    // ------------------------------------------------------------------
    // The graph
    // ------------------------------------------------------------------

    fn x_of(&self, f: f32) -> f32 {
        GRAPH.x + (f / 20.0).log10() / 3.0 * GRAPH.w
    }
    fn f_of(&self, x: f32) -> f32 {
        20.0 * 1000f32.powf(((x - GRAPH.x) / GRAPH.w).clamp(0.0, 1.0))
    }
    fn y_of(&self, db: f32) -> f32 {
        GRAPH.y + GRAPH.h * 0.5 - db / CURVE_DB * GRAPH.h * 0.5
    }
    fn db_of(&self, y: f32) -> f32 {
        (GRAPH.y + GRAPH.h * 0.5 - y) / (GRAPH.h * 0.5) * CURVE_DB
    }

    fn graph(&mut self, ui: &mut Ui) {
        let t = ui.theme;
        let sr = self.sample_rate(ui);

        ui.painter.rect_filled(GRAPH, 10.0, t.well_deep);

        // Frequency grid + labels along the bottom.
        for (f, label) in [
            (50.0, ""),
            (100.0, "100"),
            (200.0, ""),
            (500.0, ""),
            (1000.0, "1k"),
            (2000.0, ""),
            (5000.0, ""),
            (10_000.0, "10k"),
        ] {
            let x = self.x_of(f);
            ui.painter.line(x, GRAPH.y + 2.0, x, GRAPH.y + GRAPH.h - 2.0, 1.0, t.track);
            if !label.is_empty() {
                ui.label_centered(label, x, GRAPH.y + GRAPH.h - 26.0, 17.0, t.text_dim);
            }
        }
        // dB grid; 0 dB firm.
        for db in [-12.0f32, -6.0, 6.0, 12.0] {
            let y = self.y_of(db);
            ui.painter.line(GRAPH.x + 2.0, y, GRAPH.x + GRAPH.w - 2.0, y, 1.0, t.track);
        }
        let y0 = self.y_of(0.0);
        ui.painter.line(GRAPH.x + 2.0, y0, GRAPH.x + GRAPH.w - 2.0, y0, 2.0, t.text_dim);

        ui.painter.push_clip(GRAPH.inset(2.0));

        // Spectrum: smoothed bars from the bottom, -90..0 dBFS full height.
        let bin_w = GRAPH.w / SPEC_BINS as f32;
        for i in 0..SPEC_BINS {
            let target = ui.params.meter(M_SPECTRUM + i) as f32;
            let s = &mut self.spec[i];
            let coef = if target > *s { 0.5 } else { 0.06 };
            *s += (target - *s) * coef;
            if *s <= -89.0 {
                continue;
            }
            let hf = (*s + 90.0) / 90.0;
            let x = GRAPH.x + i as f32 * bin_w;
            let top = GRAPH.y + GRAPH.h * (1.0 - hf);
            ui.painter.rect_filled(
                Rect::new(x + 0.5, top, bin_w - 1.0, GRAPH.y + GRAPH.h - top),
                1.0,
                t.text_dim.with_alpha(0.16),
            );
        }

        // Band curves + combined curve, from the filters' own math.
        let coeffs: Vec<(bool, biquad::Coeffs)> = (0..BANDS)
            .map(|b| {
                let enabled = ui.params.normalized(p_enabled(b)) >= 0.5;
                let gain = if self.has_gain(ui, b) {
                    ui.params.plain(p_gain(b)) as f32
                } else {
                    0.0
                };
                let c = biquad::coeffs(
                    self.mode(ui, b),
                    ui.params.plain(p_freq(b)) as f32,
                    gain,
                    ui.params.plain(p_q(b)) as f32,
                    sr,
                );
                (enabled, c)
            })
            .collect();

        let steps = 192;
        let mut prev: [(f32, f32); BANDS] = [(0.0, 0.0); BANDS];
        let mut prev_total = (0.0f32, 0.0f32);
        for s in 0..=steps {
            let x = GRAPH.x + s as f32 / steps as f32 * GRAPH.w;
            let f = self.f_of(x);
            let mut total = 0.0;
            for (b, (enabled, c)) in coeffs.iter().enumerate() {
                if !enabled {
                    continue;
                }
                let db = biquad::mag_db(c, f, sr);
                total += db;
                let y = self.y_of(db);
                if s > 0 {
                    ui.painter.line(
                        prev[b].0,
                        prev[b].1,
                        x,
                        y,
                        2.0,
                        band_color(b).with_alpha(if b == self.selected { 0.85 } else { 0.4 }),
                    );
                }
                prev[b] = (x, y);
            }
            let ty = self.y_of(total);
            if s > 0 {
                ui.painter.line(prev_total.0, prev_total.1, x, ty, 3.0, t.text);
            }
            prev_total = (x, ty);
        }

        // Handles.
        let (mx, my) = ui.mouse_pos();
        let mut hover: Option<usize> = None;
        for b in 0..BANDS {
            let f = ui.params.plain(p_freq(b)) as f32;
            let g = if self.has_gain(ui, b) {
                ui.params.plain(p_gain(b)) as f32
            } else {
                0.0
            };
            let (hx, hy) = (self.x_of(f), self.y_of(g));
            if ((mx - hx).powi(2) + (my - hy).powi(2)).sqrt() < 16.0 {
                hover = Some(b);
            }
            let color = band_color(b);
            let enabled = ui.params.normalized(p_enabled(b)) >= 0.5;
            if enabled {
                ui.painter.circle_filled(hx, hy, 9.0, color);
            } else {
                ui.painter.circle_stroke(hx, hy, 9.0, 2.5, color.with_alpha(0.6));
            }
            if b == self.selected {
                ui.painter.circle_stroke(hx, hy, 14.0, 2.0, color.with_alpha(0.8));
            }
        }

        ui.painter.pop_clip();
        ui.painter.rect_border(GRAPH, 10.0, 3.0, t.panel_border);

        // --- Interaction ---
        if ui.mouse_clicked() {
            if let Some(b) = hover {
                self.selected = b;
                self.dragging = Some(b);
                ui.params.begin_edit(p_freq(b));
                ui.params.begin_edit(p_gain(b));
            }
        }
        if let Some(b) = self.dragging {
            let f = self.f_of(mx).clamp(20.0, 20_000.0);
            ui.params
                .perform_edit(p_freq(b), (f as f64 / 20.0).log10() / 3.0);
            if self.has_gain(ui, b) {
                let g = self.db_of(my).clamp(-15.0, 15.0);
                ui.params.perform_edit(p_gain(b), (g as f64 + 15.0) / 30.0);
            }
            if ui.mouse_released() {
                ui.params.end_edit(p_freq(b));
                ui.params.end_edit(p_gain(b));
                self.dragging = None;
            }
        } else if GRAPH.contains(mx, my) && ui.wheel_delta() != 0.0 {
            // Wheel over the graph: Q of the hovered (or selected) band.
            let b = hover.unwrap_or(self.selected);
            let q = p_q(b);
            let value = ui.params.normalized(q) + 0.04 * ui.wheel_delta() as f64;
            ui.params.begin_edit(q);
            ui.params.perform_edit(q, value);
            ui.params.end_edit(q);
        }
    }

    // ------------------------------------------------------------------
    // Band row: color chip + enable toggle per band, type switches for 1/5
    // ------------------------------------------------------------------

    fn band_row(&mut self, ui: &mut Ui) {
        let row_y = 700.0;
        let gap = 56.0;
        let mut x = GRAPH.x;

        for b in 0..BANDS {
            let toggle = Rect::new(x, row_y, 80.0, 48.0);
            let chip_alpha = if b == self.selected { 1.0 } else { 0.45 };
            ui.painter.rect_filled(
                Rect::new(x + 24.0, row_y - 12.0, 32.0, 6.0),
                3.0,
                band_color(b).with_alpha(chip_alpha),
            );
            let label = ["1", "2", "3", "4", "5"][b];
            ui.toggle(p_enabled(b), toggle, label);
            if ui.mouse_clicked() {
                let (mx, my) = ui.mouse_pos();
                if toggle.contains(mx, my) {
                    self.selected = b;
                }
            }
            x += 84.0;

            if b == 0 || b == 4 {
                let ty = if b == 0 { P_TYPE_LO } else { P_TYPE_HI };
                let filt = if b == 0 { "HPF" } else { "LPF" };
                ui.choice(ty, 0, Rect::new(x, row_y, 78.0, 48.0), filt);
                ui.choice(ty, 1, Rect::new(x + 82.0, row_y, 78.0, 48.0), "SHELF");
                x += 164.0;
            } else {
                x -= 4.0; // toggles alone: width 80 only
            }
            x += gap;
        }
    }
}
