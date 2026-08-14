//! Lantern Compressor — a character compressor with a drive stage.
//!
//! Signal flow:
//!   input ─┬─> sidechain tap ─> HPF ─> stereo-linked peak detect
//!          │                             │
//!          │                    soft-knee gain computer (dB domain)
//!          │                             │
//!          │              attack / release ballistics
//!          │                             │
//!          └─> VCA (gain reduction) ─> drive (soft clip) ─> makeup ─> mix
//!
//! The v1 DSP survives intact from the nih-plug era; the plumbing is now
//! entirely lantern_vst3 + lantern_ui. Live gain reduction flows to the
//! editor's needle meter through meter slot 0.

use lantern_ui::lantern_gpu::Rect;
use lantern_ui::{Theme, UiEditor};
use lantern_vst3::plugin::{
    Dsp, Editor, EditorFactory, MeterStore, ParamDef, ParamValues, ParamsHandle, PluginInfo,
};
use std::f32::consts::TAU;

// Parameter indices (order in PARAMS; IDs match, and are forever).
const P_THRESHOLD: usize = 0;
const P_RATIO: usize = 1;
const P_KNEE: usize = 2;
const P_ATTACK: usize = 3;
const P_RELEASE: usize = 4;
const P_SC_HPF: usize = 5;
const P_DRIVE: usize = 6;
const P_MAKEUP: usize = 7;
const P_MIX: usize = 8;

/// Meter slots (pub so the preview harness can stage demo values).
pub const M_GAIN_REDUCTION: usize = 0;
/// Output level envelope (instant attack, ~20 dB / 1.5 s fall).
pub const M_LEVEL_L: usize = 1;
pub const M_LEVEL_R: usize = 2;
/// Held peak, max-since-reset: the editor zeroes these on click, so the
/// audio thread max-accumulates into the slot instead of keeping a copy.
pub const M_PEAK_L: usize = 3;
pub const M_PEAK_R: usize = 4;
/// RMS (~300 ms exponentially-weighted window).
pub const M_RMS_L: usize = 5;
pub const M_RMS_R: usize = 6;

// ============================================================================
// Parameter mappings (normalized 0..1 <-> display values)
// ============================================================================

fn thr_plain(n: f64) -> f64 {
    n * 60.0 - 60.0
}
fn thr_norm(p: f64) -> f64 {
    (p + 60.0) / 60.0
}
fn thr_fmt(n: f64) -> String {
    format!("{:+.1}", thr_plain(n))
}

fn ratio_plain(n: f64) -> f64 {
    1.0 + 19.0 * n * n
}
fn ratio_norm(p: f64) -> f64 {
    (((p - 1.0) / 19.0).max(0.0)).sqrt()
}
fn ratio_fmt(n: f64) -> String {
    format!("{:.1}:1", ratio_plain(n))
}

fn knee_plain(n: f64) -> f64 {
    n * 24.0
}
fn knee_norm(p: f64) -> f64 {
    p / 24.0
}
fn knee_fmt(n: f64) -> String {
    format!("{:.1}", knee_plain(n))
}

fn attack_plain(n: f64) -> f64 {
    0.05 * 2000f64.powf(n) // 0.05 .. 100 ms, log
}
fn attack_norm(p: f64) -> f64 {
    (p.max(0.05) / 0.05).ln() / 2000f64.ln()
}
fn attack_fmt(n: f64) -> String {
    let p = attack_plain(n);
    if p < 10.0 {
        format!("{p:.2}")
    } else {
        format!("{p:.1}")
    }
}

fn release_plain(n: f64) -> f64 {
    20.0 * 100f64.powf(n) // 20 .. 2000 ms, log
}
fn release_norm(p: f64) -> f64 {
    (p.max(20.0) / 20.0).ln() / 100f64.ln()
}
fn release_fmt(n: f64) -> String {
    format!("{:.0}", release_plain(n))
}

fn schpf_plain(n: f64) -> f64 {
    20.0 * 25f64.powf(n) // 20 .. 500 Hz, log
}
fn schpf_norm(p: f64) -> f64 {
    (p.max(20.0) / 20.0).ln() / 25f64.ln()
}
fn schpf_fmt(n: f64) -> String {
    if n < 0.005 {
        "Off".to_string()
    } else {
        format!("{:.0} Hz", schpf_plain(n))
    }
}

fn drive_plain(n: f64) -> f64 {
    n * 24.0
}
fn drive_norm(p: f64) -> f64 {
    p / 24.0
}
fn drive_fmt(n: f64) -> String {
    format!("{:.1}", drive_plain(n))
}

fn makeup_plain(n: f64) -> f64 {
    n * 36.0 - 12.0
}
fn makeup_norm(p: f64) -> f64 {
    (p + 12.0) / 36.0
}
fn makeup_fmt(n: f64) -> String {
    format!("{:+.1}", makeup_plain(n))
}

fn mix_plain(n: f64) -> f64 {
    n * 100.0
}
fn mix_norm(p: f64) -> f64 {
    p / 100.0
}
fn mix_fmt(n: f64) -> String {
    format!("{:.0}", mix_plain(n))
}

// ============================================================================
// Sidechain high-pass (RBJ biquad, Butterworth Q)
// ============================================================================

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn set_highpass(&mut self, freq: f32, sample_rate: f32) {
        let w0 = TAU * (freq / sample_rate).min(0.49);
        let (sin, cos) = w0.sin_cos();
        // Butterworth: Q = 1/sqrt(2).
        let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        self.b0 = (1.0 + cos) / (2.0 * a0);
        self.b1 = -(1.0 + cos) / a0;
        self.b2 = self.b0;
        self.a1 = (-2.0 * cos) / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

// ============================================================================
// The DSP
// ============================================================================

pub struct CompressorDsp {
    sample_rate: f32,
    sc_filters: [Biquad; 2],
    sc_filter_freq: f32,
    /// Current gain reduction in dB (>= 0: the amount being taken off).
    gr_db: f32,
    // One-pole smoothed control values (zipper-noise control).
    sm_threshold: f32,
    sm_drive: f32,
    sm_makeup: f32,
    sm_mix: f32,
    smooth_coef: f32,
    // Output level metering.
    env_l: f32,
    env_r: f32,
    env_decay: f32,
    ms_l: f32,
    ms_r: f32,
    ms_coef: f32,
}

impl Dsp for CompressorDsp {
    const INFO: PluginInfo = PluginInfo {
        name: "Lantern Compressor",
        vendor: "Alva",
        version: "0.2.0",
        url: "https://github.com/",
        email: "noreply@example.com",
        class_id: *b"LanternComp2Alva",
        subcategories: "Fx|Dynamics",
    };

    const PARAMS: &'static [ParamDef] = &[
        ParamDef {
            id: 0,
            title: "Threshold",
            short_title: "Thresh",
            units: "dB",
            default_normalized: 1.0, // 0 dB — pull down to compress
            step_count: 0,
            can_automate: true,
            to_plain: Some(thr_plain),
            from_plain: Some(thr_norm),
            format: Some(thr_fmt),
        },
        ParamDef {
            id: 1,
            title: "Ratio",
            short_title: "Ratio",
            units: "",
            default_normalized: 0.397, // ~4:1
            step_count: 0,
            can_automate: true,
            to_plain: Some(ratio_plain),
            from_plain: Some(ratio_norm),
            format: Some(ratio_fmt),
        },
        ParamDef {
            id: 2,
            title: "Knee",
            short_title: "Knee",
            units: "dB",
            default_normalized: 0.25, // 6 dB
            step_count: 0,
            can_automate: true,
            to_plain: Some(knee_plain),
            from_plain: Some(knee_norm),
            format: Some(knee_fmt),
        },
        ParamDef {
            id: 3,
            title: "Attack",
            short_title: "Atk",
            units: "ms",
            default_normalized: 0.606, // ~5 ms
            step_count: 0,
            can_automate: true,
            to_plain: Some(attack_plain),
            from_plain: Some(attack_norm),
            format: Some(attack_fmt),
        },
        ParamDef {
            id: 4,
            title: "Release",
            short_title: "Rel",
            units: "ms",
            default_normalized: 0.437, // ~150 ms
            step_count: 0,
            can_automate: true,
            to_plain: Some(release_plain),
            from_plain: Some(release_norm),
            format: Some(release_fmt),
        },
        ParamDef {
            id: 6,
            title: "SC HPF",
            short_title: "SC HPF",
            units: "",
            default_normalized: 0.431, // ~80 Hz
            step_count: 0,
            can_automate: true,
            to_plain: Some(schpf_plain),
            from_plain: Some(schpf_norm),
            format: Some(schpf_fmt),
        },
        ParamDef {
            id: 7,
            title: "Drive",
            short_title: "Drive",
            units: "dB",
            default_normalized: 0.0,
            step_count: 0,
            can_automate: true,
            to_plain: Some(drive_plain),
            from_plain: Some(drive_norm),
            format: Some(drive_fmt),
        },
        ParamDef {
            id: 8,
            title: "Makeup",
            short_title: "Makeup",
            units: "dB",
            default_normalized: 1.0 / 3.0, // 0 dB
            step_count: 0,
            can_automate: true,
            to_plain: Some(makeup_plain),
            from_plain: Some(makeup_norm),
            format: Some(makeup_fmt),
        },
        ParamDef {
            id: 10,
            title: "Mix",
            short_title: "Mix",
            units: "%",
            default_normalized: 1.0,
            step_count: 0,
            can_automate: true,
            to_plain: Some(mix_plain),
            from_plain: Some(mix_norm),
            format: Some(mix_fmt),
        },
    ];

    const METERS: usize = 7;
    const EDITOR: Option<EditorFactory> = Some(make_editor);

    fn new() -> Self {
        Self {
            sample_rate: 44_100.0,
            sc_filters: [Biquad::default(); 2],
            sc_filter_freq: -1.0,
            gr_db: 0.0,
            sm_threshold: -18.0,
            sm_drive: 0.0,
            sm_makeup: 0.0,
            sm_mix: 1.0,
            smooth_coef: 0.0,
            env_l: 0.0,
            env_r: 0.0,
            env_decay: 1.0,
            ms_l: 0.0,
            ms_r: 0.0,
            ms_coef: 0.0,
        }
    }

    fn setup(&mut self, sample_rate: f64, _max_block_size: usize) {
        self.sample_rate = sample_rate as f32;
        self.sc_filter_freq = -1.0; // force filter recompute
        // ~20 ms control smoothing.
        self.smooth_coef = 1.0 - (-1.0 / (self.sample_rate * 0.02)).exp();
        // Meter ballistics: -20 dB / 1.5 s fall, ~300 ms RMS window.
        self.env_decay = (0.1f32.ln() / (1.5 * self.sample_rate)).exp();
        self.ms_coef = 1.0 - (-1.0 / (self.sample_rate * 0.3)).exp();
    }

    fn reset(&mut self) {
        for f in &mut self.sc_filters {
            f.reset();
        }
        self.gr_db = 0.0;
        self.env_l = 0.0;
        self.env_r = 0.0;
        self.ms_l = 0.0;
        self.ms_r = 0.0;
    }

    fn process(&mut self, buffers: &mut [&mut [f32]], params: &ParamValues, meters: &MeterStore) {
        let sr = self.sample_rate;

        // Per-block control values.
        let ratio = params.plain(P_RATIO) as f32;
        let knee = params.plain(P_KNEE) as f32;
        let attack_s = params.plain(P_ATTACK) as f32 * 1e-3;
        let release_s = params.plain(P_RELEASE) as f32 * 1e-3;
        let threshold_target = params.plain(P_THRESHOLD) as f32;
        let drive_target = params.plain(P_DRIVE) as f32;
        let makeup_target = params.plain(P_MAKEUP) as f32;
        let mix_target = (params.plain(P_MIX) as f32) / 100.0;

        let atk_coef = 1.0 - (-1.0 / (sr * attack_s)).exp();
        let rel_coef = 1.0 - (-1.0 / (sr * release_s)).exp();

        // Knob fully down = detector listens to the raw signal.
        let sc_bypass = params.normalized(P_SC_HPF) < 0.005;
        let sc_freq = params.plain(P_SC_HPF) as f32;
        if !sc_bypass && sc_freq != self.sc_filter_freq {
            for f in &mut self.sc_filters {
                f.set_highpass(sc_freq, sr);
            }
            self.sc_filter_freq = sc_freq;
        }

        let slope = 1.0 - 1.0 / ratio;
        let half_knee = knee * 0.5;

        let (first, rest) = buffers.split_at_mut(1);
        let ch_l = &mut *first[0];
        let mut ch_r = rest.first_mut();
        let num_samples = ch_l.len();

        let mut block_peak_l = 0.0f32;
        let mut block_peak_r = 0.0f32;

        for i in 0..num_samples {
            // Smoothed control values.
            self.sm_threshold += (threshold_target - self.sm_threshold) * self.smooth_coef;
            self.sm_drive += (drive_target - self.sm_drive) * self.smooth_coef;
            self.sm_makeup += (makeup_target - self.sm_makeup) * self.smooth_coef;
            self.sm_mix += (mix_target - self.sm_mix) * self.smooth_coef;

            let in_l = ch_l[i];
            let in_r = ch_r.as_ref().map(|c| c[i]).unwrap_or(in_l);

            // --- Detection: stereo-linked peak of the (HPF'd) sidechain ---
            let det = if sc_bypass {
                in_l.abs().max(in_r.abs())
            } else {
                let det_l = self.sc_filters[0].process(in_l);
                let det_r = self.sc_filters[1].process(in_r);
                det_l.abs().max(det_r.abs())
            };

            // --- Gain computer: soft-knee static curve in dB ---
            let x_db = 20.0 * det.max(1e-6).log10();
            let over = x_db - self.sm_threshold;
            let gr_target = if over <= -half_knee {
                0.0
            } else if over >= half_knee {
                slope * over
            } else {
                let t = over + half_knee;
                slope * t * t / (2.0 * knee).max(1e-9)
            };

            // --- Ballistics (smoothed in the dB domain) ---
            let coef = if gr_target > self.gr_db {
                atk_coef
            } else {
                rel_coef
            };
            self.gr_db += (gr_target - self.gr_db) * coef;

            // --- Gains ---
            let vca = 10f32.powf(-self.gr_db / 20.0);
            let makeup_gain = 10f32.powf(self.sm_makeup / 20.0);

            // Drive: ceiling-normalized tanh, faded in over the first 3 dB
            // so Drive = 0 is bit-transparent.
            let drive_amt = (self.sm_drive / 3.0).min(1.0);
            let drive_gain = 10f32.powf(self.sm_drive / 20.0);
            let drive_norm = 1.0 / drive_gain.tanh();
            let mix = self.sm_mix;

            let shape = |x: f32| -> f32 {
                let wet = x * vca;
                let driven = if drive_amt > 0.0 {
                    let shaped = (wet * drive_gain).tanh() * drive_norm;
                    wet + (shaped - wet) * drive_amt
                } else {
                    wet
                };
                let processed = driven * makeup_gain;
                x + (processed - x) * mix
            };

            let out_l = shape(in_l);
            ch_l[i] = out_l;
            let out_r = if let Some(ch_r) = ch_r.as_mut() {
                let o = shape(in_r);
                ch_r[i] = o;
                o
            } else {
                out_l
            };

            // --- Output metering: instant attack, exponential fall ---
            let (al, ar) = (out_l.abs(), out_r.abs());
            self.env_l = if al > self.env_l { al } else { self.env_l * self.env_decay };
            self.env_r = if ar > self.env_r { ar } else { self.env_r * self.env_decay };
            block_peak_l = block_peak_l.max(al);
            block_peak_r = block_peak_r.max(ar);
            self.ms_l += (out_l * out_l - self.ms_l) * self.ms_coef;
            self.ms_r += (out_r * out_r - self.ms_r) * self.ms_coef;
        }

        meters.set(M_GAIN_REDUCTION, self.gr_db as f64);
        meters.set(M_LEVEL_L, self.env_l as f64);
        meters.set(M_LEVEL_R, self.env_r as f64);
        meters.set(M_PEAK_L, meters.get(M_PEAK_L).max(block_peak_l as f64));
        meters.set(M_PEAK_R, meters.get(M_PEAK_R).max(block_peak_r as f64));
        meters.set(M_RMS_L, self.ms_l.sqrt() as f64);
        meters.set(M_RMS_R, self.ms_r.sqrt() as f64);
    }
}

lantern_vst3::export_plugin!(CompressorDsp);

// ============================================================================
// The face: GR needle hero up top with Threshold + Makeup front and center
// beneath it; detector controls (timing, curve, sidechain filter) on the
// left; output meter with the character controls on the right
// ============================================================================

fn make_editor(params: ParamsHandle) -> Box<dyn Editor> {
    UiEditor::create(1200, 790, Theme::lantern(), params, |ui| draw_face(ui))
}

/// The whole face derives from two rects: the GR box and the bottom row.
/// Every other rect is a strip, column, or row of those, so the columns
/// share edges by construction.
fn draw_face(ui: &mut lantern_ui::Ui) {
    ui.face();
    let t = ui.theme;

    let gr_box = Rect::new(334.0, 44.0, 532.0, 440.0);
    let bottom_row = Rect::new(gr_box.x, 524.0, gr_box.w, 222.0);

    // Hero: needle meter, divider tight beneath, Threshold + Makeup front
    // and center on the bottom row.
    ui.gr_meter(0, ui.params.meter(M_GAIN_REDUCTION), gr_box);
    ui.divider(gr_box.x, gr_box.y + gr_box.h + 20.0, gr_box.w);
    ui.knob_cell(P_THRESHOLD, Rect::new(400.0, bottom_row.y, 140.0, bottom_row.h), "THRESHOLD");
    ui.knob_cell(P_MAKEUP, Rect::new(660.0, bottom_row.y, 140.0, bottom_row.h), "MAKEUP");

    // Left: the detector 2x2, hung on the GR box's vertical span.
    let left = Rect::new(44.0, gr_box.y, 242.0, gr_box.h);
    for (param, label, c, r) in [
        (P_ATTACK, "ATTACK", 0u32, 0u32),
        (P_RELEASE, "RELEASE", 1, 0),
        (P_RATIO, "RATIO", 0, 1),
        (P_KNEE, "KNEE", 1, 1),
    ] {
        ui.knob_cell(param, left.col(c, 2, 18.0).row(r, 2, 52.0), label);
    }

    // Between GR and the output meter: character stack on the same rows,
    // sidechain filter bottoming out on the bottom row's baseline.
    let mid = Rect::new(916.0, gr_box.y, 112.0, gr_box.h);
    ui.knob_cell(P_DRIVE, mid.row(0, 2, 52.0), "DRIVE");
    ui.knob_cell(P_MIX, mid.row(1, 2, 52.0), "MIX");
    let hpf = Rect::new(mid.x, bottom_row.y + bottom_row.h - 68.0, mid.w, 68.0);
    ui.label_centered("SC HPF", hpf.center_x(), hpf.y, 21.0, t.text_dim);
    ui.drag_box(P_SC_HPF, hpf.strip_bottom(34.0));

    // Output meter: full height, GR top line to the bottom row's baseline.
    ui.level_meter(
        Rect::new(1078.0, gr_box.y, 80.0, gr_box.spanning_to(&bottom_row).h),
        (M_LEVEL_L, M_LEVEL_R),
        (M_PEAK_L, M_PEAK_R),
        (M_RMS_L, M_RMS_R),
    );
}

/// The face closure for the preview harness (no host, no window).
pub fn preview_face() -> impl FnMut(&mut lantern_ui::Ui) {
    |ui| draw_face(ui)
}
