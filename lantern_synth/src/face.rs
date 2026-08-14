//! The Blacklight face, rebuilt from a clean slate: controls move in one
//! round at a time, checked in the preview harness as they land.
//!
//! Current state: three sub-panel rooms across the top (OSC 1, OSC 2,
//! FILTER, no titles), three below the fire line (ENVELOPES with AMP/FLT
//! tabs and the live ADSR curve, the OUT room with the drive curve, FM |
//! DRIVE | GAIN cord targets, voice dots, and the family output meter,
//! then the WOBBLE room), and a playable keyboard spanning the whole
//! bottom — click to sound notes (drag = glissando), keys light for
//! whatever the voices are sounding, MIDI included, out-of-range notes
//! folding into view by octaves.

use lantern_ui::lantern_gpu::{BackgroundImage, Color, Rect};
use lantern_ui::{Theme, Ui, UiEditor};
use lantern_vst3::plugin::{Editor, ParamsHandle};

use crate::synth::{osc, svf_k, FilterMode, Waveform};
use crate::{
    p_lfo_rate, p_lfo_step, p_mod_amt, p_mod_dest, p_mod_src, p_osc, D_CUTOFF, D_DRIVE, D_FM,
    D_GAIN, LFOS, LFO_STEPS, MOD_DESTS, MOD_SLOTS, M_KEYS, M_LEVEL_L, M_LEVEL_R, M_LFO_PHASE,
    M_PEAK_L, M_PEAK_R, M_RMS_L, M_RMS_R, M_UI_NOTE, M_VOICES, NUM_VOICES, OSC_DETUNE, OSC_ENABLE,
    OSC_PITCH, OSC_VOICES, OSC_VOLUME, OSC_WAVE, P_AMP_A, P_AMP_D, P_AMP_R, P_AMP_S, P_CUTOFF,
    P_DRIVE, P_FLT_A, P_FLT_D, P_FLT_KEYTRACK, P_FLT_ON, P_FLT_R, P_FLT_S, P_FLT_TYPE,
    P_LFO_DEPTH, P_RES, P_WIDTH,
};

const WIN_W: u32 = 1700;
const WIN_H: u32 = 1176;

/// Keyboard range: C1 to C5 in Live's naming — C3, Alva's home row, dead
/// center, keys at proper piano proportions.
const KB_LOW: u8 = 36;
const WHITE_KEYS: usize = 29; // 4 octaves + the top C

/// Alva's fire, embedded raw (850x588 RGBA — fire_background_3, center-
/// cropped to the window's aspect so the stretch stays undistorted),
/// drawn under the translucent panels.
static BG_RGBA: &[u8] = include_bytes!("../assets/bg.rgba");

pub fn background() -> BackgroundImage {
    BackgroundImage {
        rgba: BG_RGBA.to_vec(),
        width: 850,
        height: 588,
        alpha: 1.0,
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

/// Preview variant with the WOBBLE room flipped to the MODS list.
pub fn preview_face_mods() -> impl FnMut(&mut Ui) {
    let mut face = Face {
        show_mods: true,
        ..Face::default()
    };
    move |ui| face.draw(ui)
}

#[derive(Default)]
struct Face {
    /// Key currently held by the mouse (MIDI note).
    held: Option<u8>,
    /// Which LFO tab is showing.
    active_lfo: usize,
    /// Which envelope tab is showing: 0 = amp, 1 = filter.
    active_env: usize,
    /// The WOBBLE room's MODS view: the whole matrix as an editable list.
    show_mods: bool,
    /// Mid-drag in the LFO drawing grid.
    painting: bool,
    /// Knob cells that accept a mod cord, rebuilt every frame as the
    /// panels draw: (destination index into MOD_DESTS, cell rect).
    targets: Vec<(usize, Rect)>,
    /// An LFO tab being dragged toward a knob.
    drag: Option<ModDrag>,
}

#[derive(Clone, Copy)]
struct ModDrag {
    lfo: usize,
    from: (f32, f32),
    /// The cord only comes alive after real travel — a plain click on the
    /// tab still just switches tabs.
    live: bool,
}

/// Each LFO's signature color: gold, ember, blue. Tabs, cords, chips, and
/// knob badges all speak it, so a glance answers "who is wobbling this".
fn lfo_color(t: &Theme, l: usize) -> Color {
    [t.accent, t.warm, t.cool][l.min(2)]
}

/// Route `lfo` to `dest` in the first free matrix slot (destination set
/// last, so the DSP never sees a half-configured live slot). Fresh routes
/// land at +50% — audible immediately. Duplicate drops are no-ops.
fn assign(ui: &mut Ui, lfo: usize, dest: usize) {
    let mut free = None;
    for s in 0..MOD_SLOTS {
        let d = ((ui.params.normalized(p_mod_dest(s)) * 10.0).round() as usize).min(10);
        if d == 0 {
            if free.is_none() {
                free = Some(s);
            }
        } else if d == dest {
            let src = ((ui.params.normalized(p_mod_src(s)) * 2.0).round() as usize).min(LFOS - 1);
            if src == lfo {
                return;
            }
        }
    }
    let Some(s) = free else { return }; // matrix full — eight cords is plenty
    for (p, v) in [
        (p_mod_src(s), lfo as f64 / 2.0),
        (p_mod_amt(s), 0.75),
        (p_mod_dest(s), dest as f64 / 10.0),
    ] {
        ui.params.begin_edit(p);
        ui.params.perform_edit(p, v);
        ui.params.end_edit(p);
    }
}

/// Sub-panel fill: one step lighter than the face panel.
fn sub_panel(ui: &mut Ui, rect: Rect) {
    let t = ui.theme;
    // Subtle top-to-bottom falloff: lighter at the top edge, sinking away.
    ui.painter.rect_gradient_linear(
        rect,
        10.0,
        std::f32::consts::FRAC_PI_2,
        Color::from_rgb8(28, 28, 28),
        Color::from_rgb8(19, 19, 19),
    );
    ui.painter.rect_border(rect, 10.0, 2.5, t.panel_border);
}

impl Face {
    /// The filter room: power + type dropdown on top, the live response
    /// curve beneath (drawn from the same math the audio runs), then
    /// CUTOFF | RES | ENV AMT | KEYTRK — the once-spare slot now belongs
    /// to the comb modes' note tracking.
    fn filter_panel(&mut self, ui: &mut Ui, r: Rect) {
        sub_panel(ui, r);

        ui.toggle(P_FLT_ON, Rect::new(r.x + 12.0, r.y + 14.0, 48.0, 48.0), "");
        ui.dropdown(P_FLT_TYPE, Rect::new(r.x + 68.0, r.y + 14.0, r.w - 82.0, 48.0));

        response_window(ui, Rect::new(r.x + 16.0, r.y + 74.0, r.w - 32.0, 130.0));

        let start = r.x + (r.w - 468.0) * 0.5;
        let cell = |i: usize| Rect::new(start + i as f32 * 118.0, r.y + 216.0, 114.0, 190.0);
        for (i, (dest, label)) in [(D_CUTOFF, "CUTOFF"), (D_CUTOFF + 1, "RES"), (D_CUTOFF + 2, "ENV AMT")]
            .iter()
            .enumerate()
        {
            self.dest_knob(ui, *dest, cell(i), label);
        }
        let kt = cell(3);
        let t = ui.theme;
        ui.label_centered("KEYTRK", kt.center_x(), kt.y, 21.0, t.text_dim);
        ui.toggle(
            P_FLT_KEYTRACK,
            Rect::new(kt.center_x() - 36.0, kt.y + 68.0, 72.0, 48.0),
            "KEY",
        );
    }

    /// A knob that the mod matrix can reach: draws like any knob cell and
    /// registers itself as a cord drop target for this frame.
    fn dest_knob(&mut self, ui: &mut Ui, dest: usize, cell: Rect, label: &str) {
        ui.knob_cell(MOD_DESTS[dest].0, cell, label);
        self.targets.push((dest, cell));
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
    let mode = FilterMode::from_index((ui.params.normalized(P_FLT_TYPE) * 5.0).round() as usize);
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
            // Feedback comb teeth: |H| = 1 / |1 -/+ g e^{-j 2π f/fc}|.
            FilterMode::CombPlus | FilterMode::CombMinus => {
                let g = 0.98 * res;
                let c = (std::f32::consts::TAU * x_rel).cos();
                let sign = if mode == FilterMode::CombPlus { -1.0 } else { 1.0 };
                1.0 / (1.0 + g * g + 2.0 * g * c * sign).max(1e-6).sqrt()
            }
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

/// The envelope window: both ADSR curves live from the params — the active
/// one bright, its sibling ghosted behind for context. Segment widths ride
/// the knobs' own log-time mapping (a longer attack IS a wider ramp);
/// sustain holds a fixed berth so the shape always reads, and the faint
/// vertical is note-off.
fn env_window(ui: &mut Ui, rect: Rect, active: usize) {
    let t = ui.theme;
    ui.painter.rect_filled(rect, 8.0, t.well_deep);

    let pad = 14.0;
    let (w, h) = (rect.w - pad * 2.0, rect.h - pad * 2.0);
    let (x0, y0) = (rect.x + pad, rect.y + rect.h - pad);

    let envs = [
        [P_AMP_A, P_AMP_D, P_AMP_S, P_AMP_R],
        [P_FLT_A, P_FLT_D, P_FLT_S, P_FLT_R],
    ];
    ui.painter.push_clip(rect.inset(3.0));
    for pass in 0..2 {
        let e = if pass == 0 { 1 - active } else { active };
        let [pa, pd, ps, pr] = envs[e];
        let na = ui.params.normalized(pa) as f32;
        let nd = ui.params.normalized(pd) as f32;
        let s = (ui.params.plain(ps) as f32 / 100.0).clamp(0.0, 1.0);
        let nr = ui.params.normalized(pr) as f32;
        let (wa, wd, ws, wr) = (0.06 + na, 0.06 + nd, 0.42, 0.06 + nr);
        let total = wa + wd + ws + wr;
        let xs = [
            x0,
            x0 + wa / total * w,
            x0 + (wa + wd) / total * w,
            x0 + (wa + wd + ws) / total * w,
            x0 + w,
        ];
        let ys = [y0, y0 - h, y0 - h * s, y0 - h * s, y0];
        if pass == 1 {
            ui.painter
                .line(xs[3], rect.y + 6.0, xs[3], rect.y + rect.h - 6.0, 1.0, t.track);
        }
        let (color, width) = if pass == 1 {
            (t.accent, 2.5)
        } else {
            (t.text_dim.with_alpha(0.35), 2.0)
        };
        for i in 0..4 {
            ui.painter.line(xs[i], ys[i], xs[i + 1], ys[i + 1], width, color);
        }
        if pass == 1 {
            for i in 1..4 {
                ui.painter.circle_filled(xs[i], ys[i], 4.0, color);
            }
        }
    }
    ui.painter.pop_clip();
    ui.painter.rect_border(rect, 8.0, 2.5, t.panel_border);
}

/// The drive window: the output stage's transfer curve, from the very tanh
/// the samples run through — the dim diagonal is unity, the gold curve is
/// what the drive knob does to it.
fn drive_window(ui: &mut Ui, rect: Rect) {
    let t = ui.theme;
    ui.painter.rect_filled(rect, 8.0, t.well_deep);

    let y_mid = rect.center_y();
    ui.painter.line(rect.x + 6.0, y_mid, rect.x + rect.w - 6.0, y_mid, 1.0, t.track);

    let pad = 14.0;
    let w = rect.w - pad * 2.0;
    let half = rect.h * 0.40;
    ui.painter.line(
        rect.x + pad,
        y_mid + half,
        rect.x + pad + w,
        y_mid - half,
        1.0,
        t.track,
    );

    let g = 1.0 + ui.params.plain(P_DRIVE) as f32 / 100.0 * 24.0;
    let n = 96;
    let mut prev: Option<(f32, f32)> = None;
    ui.painter.push_clip(rect.inset(3.0));
    for i in 0..=n {
        let xn = i as f32 / n as f32 * 2.0 - 1.0;
        let x = rect.x + pad + (xn * 0.5 + 0.5) * w;
        let y = y_mid - (xn * g).tanh() * half;
        if let Some((px, py)) = prev {
            ui.painter.line(px, py, x, y, 2.5, t.accent);
        }
        prev = Some((x, y));
    }
    ui.painter.pop_clip();
    ui.painter.rect_border(rect, 8.0, 2.5, t.panel_border);
}

impl Face {
    /// The envelope room: AMP | FILTER tabs, the live curve window (the
    /// same linear segments the voices run), and the active envelope's
    /// four knobs.
    fn env_panel(&mut self, ui: &mut Ui, r: Rect) {
        let t = ui.theme;
        sub_panel(ui, r);

        let (mx, my) = ui.mouse_pos();
        for (e, name) in ["AMP", "FILTER"].iter().enumerate() {
            let tab = Rect::new(r.x + 12.0 + e as f32 * 146.0, r.y + 14.0, 140.0, 40.0);
            let selected = e == self.active_env;
            if ui.mouse_clicked() && tab.contains(mx, my) {
                self.active_env = e;
            }
            ui.painter.rect_filled(
                tab,
                6.0,
                if selected { t.accent.with_alpha(0.25) } else { t.well_deep },
            );
            ui.painter.rect_border(
                tab,
                6.0,
                2.5,
                if selected { t.accent } else { t.panel_border },
            );
            ui.label_centered(
                name,
                tab.center_x(),
                tab.y + 8.0,
                21.0,
                if selected { t.text } else { t.text_dim },
            );
        }

        env_window(ui, Rect::new(r.x + 16.0, r.y + 66.0, r.w - 32.0, 200.0), self.active_env);

        let ps = if self.active_env == 0 {
            [P_AMP_A, P_AMP_D, P_AMP_S, P_AMP_R]
        } else {
            [P_FLT_A, P_FLT_D, P_FLT_S, P_FLT_R]
        };
        let start = r.x + (r.w - 468.0) * 0.5;
        for (i, (p, label)) in ps
            .iter()
            .zip(["ATTACK", "DECAY", "SUSTAIN", "RELEASE"])
            .enumerate()
        {
            ui.knob_cell(
                *p,
                Rect::new(start + i as f32 * 118.0, r.y + 276.0, 114.0, 186.0),
                label,
            );
        }
    }

    /// The OUT room: voice dots up top (one per voice, lit while it
    /// sounds), the drive transfer curve with the stereo WIDTH knob beside
    /// it, FM | DRIVE | GAIN — every one a cord target — and the family
    /// output meter on the right.
    fn out_panel(&mut self, ui: &mut Ui, r: Rect) {
        let t = ui.theme;
        sub_panel(ui, r);

        ui.label("VOICES", r.x + 18.0, r.y + 14.0, 19.0, t.text_dim);
        let sounding = ui.params.meter(M_VOICES) as usize;
        for v in 0..NUM_VOICES {
            let color = if v < sounding { t.accent } else { t.track };
            ui.painter
                .circle_filled(r.x + 24.0 + v as f32 * 15.0, r.y + 48.0, 4.5, color);
        }

        drive_window(ui, Rect::new(r.x + 16.0, r.y + 66.0, 232.0, 200.0));
        ui.knob_cell(P_WIDTH, Rect::new(r.x + 256.0, r.y + 66.0, 114.0, 200.0), "WIDTH");

        for (i, (dest, label)) in [(D_FM, "FM 2→1"), (D_DRIVE, "DRIVE"), (D_GAIN, "GAIN")]
            .iter()
            .enumerate()
        {
            let cell = Rect::new(r.x + 16.0 + i as f32 * 118.0, r.y + 276.0, 114.0, 186.0);
            self.dest_knob(ui, *dest, cell, label);
        }

        ui.level_meter(
            Rect::new(r.x + r.w - 114.0, r.y + 14.0, 98.0, 448.0),
            (M_LEVEL_L, M_LEVEL_R),
            (M_PEAK_L, M_PEAK_R),
            (M_RMS_L, M_RMS_R),
        );
    }

    fn draw(&mut self, ui: &mut Ui) {
        ui.face_glass(0.72);
        self.targets.clear();

        // Three wide rooms across the top: OSC 1 | OSC 2 | FILTER.
        self.osc_panel(ui, Rect::new(44.0, 44.0, 500.0, 420.0), 0);
        self.osc_panel(ui, Rect::new(600.0, 44.0, 500.0, 420.0), 1);
        self.filter_panel(ui, Rect::new(1156.0, 44.0, 500.0, 420.0));

        // The fire hairline, back where it belongs.
        ui.divider(44.0, 480.0, 1612.0);

        // Below the line: ENVELOPES | OUT | the WOBBLE room.
        self.env_panel(ui, Rect::new(44.0, 496.0, 500.0, 470.0));
        self.out_panel(ui, Rect::new(600.0, 496.0, 500.0, 470.0));
        self.lfo_panel(ui, Rect::new(1156.0, 496.0, 500.0, 470.0));

        self.mod_badges(ui);
        self.keyboard(ui, Rect::new(44.0, 982.0, 1612.0, 150.0));
        self.mod_cord(ui);
    }

    /// Three drawable step LFOs: tabs (drag one onto a knob to route it),
    /// the drawing grid, tempo-synced RATE, the classic DEPTH wobble on
    /// LFO 1, and the active tab's mod-matrix routes as editable chips.
    /// The MODS tab at top right flips the room to the full matrix list.
    fn lfo_panel(&mut self, ui: &mut Ui, r: Rect) {
        let t = ui.theme;
        sub_panel(ui, r);

        let (mx, my) = ui.mouse_pos();
        for l in 0..LFOS {
            let tab = Rect::new(r.x + 12.0 + l as f32 * 96.0, r.y + 14.0, 90.0, 40.0);
            let selected = l == self.active_lfo && !self.show_mods;
            let color = lfo_color(t, l);
            if ui.mouse_clicked() && tab.contains(mx, my) {
                self.active_lfo = l;
                self.show_mods = false;
                // Arm a drag: it becomes the assignment cord if the mouse
                // travels; a motionless click just switched tabs.
                self.drag = Some(ModDrag {
                    lfo: l,
                    from: (tab.center_x(), tab.center_y()),
                    live: false,
                });
            }
            ui.painter.rect_filled(
                tab,
                6.0,
                if selected { color.with_alpha(0.25) } else { t.well_deep },
            );
            ui.painter.rect_border(
                tab,
                6.0,
                2.5,
                if selected { color } else { t.panel_border },
            );
            ui.label_centered(
                &format!("LFO {}", l + 1),
                tab.center_x(),
                tab.y + 8.0,
                21.0,
                if selected { t.text } else { t.text_dim },
            );
        }

        // The MODS tab, top right: flip to the whole matrix as a list.
        let mtab = Rect::new(r.x + r.w - 102.0, r.y + 14.0, 90.0, 40.0);
        if ui.mouse_clicked() && mtab.contains(mx, my) {
            self.show_mods = true;
        }
        ui.painter.rect_filled(
            mtab,
            6.0,
            if self.show_mods { t.accent.with_alpha(0.25) } else { t.well_deep },
        );
        ui.painter.rect_border(
            mtab,
            6.0,
            2.5,
            if self.show_mods { t.accent } else { t.panel_border },
        );
        ui.label_centered(
            "MODS",
            mtab.center_x(),
            mtab.y + 8.0,
            21.0,
            if self.show_mods { t.text } else { t.text_dim },
        );

        if self.show_mods {
            self.mod_list(ui, Rect::new(r.x + 16.0, r.y + 66.0, r.w - 32.0, r.h - 82.0));
            return;
        }

        let l = self.active_lfo;
        self.lfo_grid(ui, Rect::new(r.x + 16.0, r.y + 66.0, r.w - 32.0, 200.0), l);

        ui.knob_cell(p_lfo_rate(l), Rect::new(r.x + 16.0, r.y + 276.0, 150.0, 186.0), "RATE");
        let chips_x = if l == 0 {
            ui.knob_cell(P_LFO_DEPTH, Rect::new(r.x + 172.0, r.y + 276.0, 150.0, 186.0), "DEPTH");
            r.x + 328.0
        } else {
            r.x + 172.0
        };
        self.route_chips(
            ui,
            l,
            Rect::new(chips_x, r.y + 276.0, r.x + r.w - 16.0 - chips_x, 186.0),
        );
    }

    /// The MODS view: all eight matrix slots as rows — source and
    /// destination dropdowns, draggable amount, an x to unplug. Free slots
    /// sit dim, holding their place, so rows never jump around.
    fn mod_list(&mut self, ui: &mut Ui, area: Rect) {
        let t = ui.theme;
        let (mx, my) = ui.mouse_pos();
        let pitch = (area.h / MOD_SLOTS as f32).min(48.0);
        for s in 0..MOD_SLOTS {
            let y = area.y + s as f32 * pitch;
            let row_h = pitch - 8.0;
            let dest = ((ui.params.normalized(p_mod_dest(s)) * 10.0).round() as usize).min(10);
            if dest == 0 {
                ui.painter.rect_filled(
                    Rect::new(area.x, y, area.w, row_h),
                    6.0,
                    t.well_deep.with_alpha(0.5),
                );
                continue;
            }
            let src = ((ui.params.normalized(p_mod_src(s)) * 2.0).round() as usize).min(LFOS - 1);
            ui.painter
                .circle_filled(area.x + 8.0, y + row_h * 0.5, 6.0, lfo_color(t, src));
            ui.dropdown(p_mod_src(s), Rect::new(area.x + 22.0, y, 108.0, row_h));
            ui.dropdown(p_mod_dest(s), Rect::new(area.x + 138.0, y, 150.0, row_h));
            ui.drag_box(p_mod_amt(s), Rect::new(area.x + 296.0, y, 120.0, row_h));
            let xr = Rect::new(area.x + 428.0, y + (row_h - 26.0) * 0.5, 26.0, 26.0);
            let hov = xr.contains(mx, my);
            if hov && ui.mouse_clicked() {
                let p = p_mod_dest(s);
                ui.params.begin_edit(p);
                ui.params.perform_edit(p, 0.0);
                ui.params.end_edit(p);
            }
            let xc = if hov { t.text } else { t.text_dim };
            ui.painter.line(xr.x + 8.0, xr.y + 8.0, xr.x + 18.0, xr.y + 18.0, 2.5, xc);
            ui.painter.line(xr.x + 18.0, xr.y + 8.0, xr.x + 8.0, xr.y + 18.0, 2.5, xc);
        }
    }

    /// The active LFO's matrix routes: one chip per route — destination,
    /// draggable bipolar amount, and an x to unplug it.
    fn route_chips(&mut self, ui: &mut Ui, l: usize, area: Rect) {
        let t = ui.theme;
        let color = lfo_color(t, l);
        let mut slots = [0usize; MOD_SLOTS];
        let mut count = 0;
        for s in 0..MOD_SLOTS {
            let dest = ((ui.params.normalized(p_mod_dest(s)) * 10.0).round() as usize).min(10);
            let src = ((ui.params.normalized(p_mod_src(s)) * 2.0).round() as usize).min(LFOS - 1);
            if dest != 0 && src == l {
                slots[count] = s;
                count += 1;
            }
        }
        if count == 0 {
            let cx = area.center_x();
            ui.label_centered("drag the tab", cx, area.y + 56.0, 21.0, t.text_dim.with_alpha(0.6));
            ui.label_centered("onto a knob", cx, area.y + 86.0, 21.0, t.text_dim.with_alpha(0.6));
            return;
        }
        let cols = ((area.w / 156.0).floor() as usize).max(1);
        let cap = cols * 2;
        let (mx, my) = ui.mouse_pos();
        for (i, &s) in slots[..count].iter().enumerate().take(cap) {
            let (col, row) = (i % cols, i / cols);
            let chip = Rect::new(
                area.x + col as f32 * 156.0,
                area.y + row as f32 * 94.0,
                150.0,
                88.0,
            );
            let dest = ((ui.params.normalized(p_mod_dest(s)) * 10.0).round() as usize).min(10);
            ui.painter.rect_filled(chip, 8.0, t.well_deep);
            ui.painter.rect_border(chip, 8.0, 2.0, color.with_alpha(0.7));
            ui.label(&format!("→ {}", MOD_DESTS[dest].1), chip.x + 8.0, chip.y + 9.0, 19.0, t.text);
            // Unplug: the little x in the corner.
            let xr = Rect::new(chip.x + chip.w - 30.0, chip.y + 4.0, 26.0, 26.0);
            let hov = xr.contains(mx, my);
            if hov && ui.mouse_clicked() {
                let p = p_mod_dest(s);
                ui.params.begin_edit(p);
                ui.params.perform_edit(p, 0.0);
                ui.params.end_edit(p);
            }
            let xc = if hov { t.text } else { t.text_dim };
            ui.painter.line(xr.x + 8.0, xr.y + 8.0, xr.x + 18.0, xr.y + 18.0, 2.5, xc);
            ui.painter.line(xr.x + 18.0, xr.y + 8.0, xr.x + 8.0, xr.y + 18.0, 2.5, xc);
            ui.drag_box(p_mod_amt(s), Rect::new(chip.x + 8.0, chip.y + 44.0, chip.w - 16.0, 34.0));
        }
        if count > cap {
            ui.label(
                &format!("+{}", count - cap),
                area.x + area.w - 36.0,
                area.y + area.h - 26.0,
                19.0,
                t.text_dim,
            );
        }
    }

    /// A colored dot on every knob the matrix — or the grandfathered DEPTH
    /// wobble — is modulating; the color names the source LFO. The dots sit
    /// in the arc's bottom gap, between the knob and its value box.
    fn mod_badges(&mut self, ui: &mut Ui) {
        let mut routed = [(0usize, 0usize); MOD_SLOTS + 1];
        let mut n = 0;
        for s in 0..MOD_SLOTS {
            let dest = ((ui.params.normalized(p_mod_dest(s)) * 10.0).round() as usize).min(10);
            if dest == 0 {
                continue;
            }
            let src = ((ui.params.normalized(p_mod_src(s)) * 2.0).round() as usize).min(LFOS - 1);
            routed[n] = (dest, src);
            n += 1;
        }
        if ui.params.plain(P_LFO_DEPTH) > 0.05 {
            routed[n] = (D_CUTOFF, 0);
            n += 1;
        }
        let t = ui.theme;
        for &(dest_target, cell) in &self.targets {
            let mut srcs = [0usize; MOD_SLOTS + 1];
            let mut k = 0;
            for &(dest, src) in &routed[..n] {
                if dest == dest_target {
                    srcs[k] = src;
                    k += 1;
                }
            }
            if k == 0 {
                continue;
            }
            // Mirror knob_cell's geometry: dot row just under the knob,
            // centered, clear of the value box.
            let frame = ((cell.h - 86.0) * 0.5).max(20.0);
            let y = cell.y + 2.0 * frame + 38.0;
            for (i, &src) in srcs[..k].iter().enumerate() {
                let x = cell.center_x() + (i as f32 - (k - 1) as f32 * 0.5) * 16.0;
                ui.painter.circle_filled(x, y, 6.0, lfo_color(t, src));
            }
        }
    }

    /// The assignment cord: drag an LFO tab and a glowing cable follows the
    /// cursor, every reachable knob outlines in the LFO's color; let go on
    /// one to plug in.
    fn mod_cord(&mut self, ui: &mut Ui) {
        let Some(mut drag) = self.drag else { return };
        let (mx, my) = ui.mouse_pos();
        let (fx, fy) = drag.from;
        if !drag.live {
            if !ui.mouse_is_down() {
                self.drag = None;
                return;
            }
            if (mx - fx).powi(2) + (my - fy).powi(2) < 196.0 {
                return;
            }
            drag.live = true;
            self.drag = Some(drag);
        }
        if !ui.mouse_is_down() {
            // Dropped: on a knob = plug in; anywhere else = never mind.
            if let Some(&(dest, _)) = self.targets.iter().find(|(_, c)| c.contains(mx, my)) {
                assign(ui, drag.lfo, dest);
            }
            self.drag = None;
            return;
        }
        let t = ui.theme;
        let color = lfo_color(t, drag.lfo);

        // Cord + target highlights ride the overlay layer, above everything.
        ui.painter.set_layer(1);
        for &(_, cell) in &self.targets {
            let hov = cell.contains(mx, my);
            if hov {
                ui.painter.rect_filled(cell, 8.0, color.with_alpha(0.10));
            }
            ui.painter
                .rect_border(cell, 8.0, 2.5, color.with_alpha(if hov { 1.0 } else { 0.4 }));
        }
        // A sagging cable from the tab to the cursor (quadratic bezier).
        let sag = ((mx - fx).abs() + (my - fy).abs()) * 0.12 + 24.0;
        let (cx, cy) = ((fx + mx) * 0.5, (fy + my) * 0.5 + sag);
        let mut prev = (fx, fy);
        for i in 1..=26 {
            let s = i as f32 / 26.0;
            let inv = 1.0 - s;
            let x = inv * inv * fx + 2.0 * inv * s * cx + s * s * mx;
            let y = inv * inv * fy + 2.0 * inv * s * cy + s * s * my;
            ui.painter.line(prev.0, prev.1, x, y, 8.0, color.with_alpha(0.18));
            ui.painter.line(prev.0, prev.1, x, y, 3.0, color);
            prev = (x, y);
        }
        ui.painter.circle_filled(fx, fy, 6.0, color);
        ui.painter.circle_filled(mx, my, 12.0, color.with_alpha(0.25));
        ui.painter.circle_filled(mx, my, 6.0, color);
        ui.painter.set_layer(0);
    }

    /// The drawing grid: 16 steps, drag across to paint the shape. The
    /// playhead sweeps at the synced rate — what you draw is what wobbles.
    fn lfo_grid(&mut self, ui: &mut Ui, grid: Rect, l: usize) {
        let t = ui.theme;
        ui.painter.rect_filled(grid, 8.0, t.well_deep);

        // Paint: click or drag sets the step under the cursor to the
        // cursor's height.
        let (mx, my) = ui.mouse_pos();
        if ui.mouse_clicked() && grid.contains(mx, my) {
            self.painting = true;
        }
        if !ui.mouse_is_down() {
            self.painting = false;
        }
        if self.painting {
            let cell = grid.w / LFO_STEPS as f32;
            let s = ((((mx - grid.x) / cell) as isize).max(0) as usize).min(LFO_STEPS - 1);
            let v = (1.0 - (my - grid.y) / grid.h).clamp(0.0, 1.0) as f64;
            let p = p_lfo_step(l, s);
            if (ui.params.normalized(p) - v).abs() > 0.004 {
                ui.params.begin_edit(p);
                ui.params.perform_edit(p, v);
                ui.params.end_edit(p);
            }
        }

        // Beat grid: quarter marks brighter, mid line faint.
        for q in 1..4 {
            let x = grid.x + grid.w * q as f32 / 4.0;
            ui.painter.line(x, grid.y + 2.0, x, grid.y + grid.h - 2.0, 1.0, t.track);
        }
        ui.painter.line(
            grid.x + 4.0,
            grid.center_y(),
            grid.x + grid.w - 4.0,
            grid.center_y(),
            1.0,
            t.track.with_alpha(0.6),
        );

        let cell = grid.w / LFO_STEPS as f32;
        for s in 0..LFO_STEPS {
            let v = ui.params.normalized(p_lfo_step(l, s)) as f32;
            let h = grid.h * v;
            let x = grid.x + s as f32 * cell;
            if h > 1.0 {
                ui.painter.rect_filled(
                    Rect::new(x + 1.5, grid.y + grid.h - h, cell - 3.0, h),
                    2.0,
                    t.accent.with_alpha(0.45),
                );
            }
            ui.painter.rect_filled(
                Rect::new(x + 1.5, (grid.y + grid.h - h - 2.0).max(grid.y), cell - 3.0, 3.5),
                1.0,
                t.accent,
            );
        }

        // Playhead, live from the DSP.
        let phase = ui.params.meter(M_LFO_PHASE + l) as f32;
        let px = grid.x + phase.clamp(0.0, 1.0) * grid.w;
        ui.painter.line(px, grid.y + 2.0, px, grid.y + grid.h - 2.0, 2.5, t.warm);

        ui.painter.rect_border(grid, 8.0, 2.5, t.panel_border);
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
            let cell = Rect::new(r.x + 16.0 + i as f32 * 118.0, r.y + 216.0, 114.0, 190.0);
            // VOLUME and DETUNE take mod cords; VOICES/PITCH are stepped
            // and stay plain knobs.
            match *which {
                OSC_VOLUME => self.dest_knob(ui, 1 + o * 2, cell, label),
                OSC_DETUNE => self.dest_knob(ui, 2 + o * 2, cell, label),
                _ => ui.knob_cell(p_osc(o, *which), cell, label),
            }
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
        'blacks: for oct in 0..4 {
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
                    &format!("C{}", i / 7 + 1),
                    r.x + 7.0,
                    r.y + r.h - 28.0,
                    17.0,
                    Color::from_rgb8(48, 48, 48),
                );
            }
        }
        for oct in 0..4 {
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
