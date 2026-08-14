//! Lantern Waveshaper — the saturation↔distortion continuum in one box.
//!
//! Signal flow, per channel:
//!   input ─┬─────────────────────────────────────────────┐ (dry)
//!          ├─> [SUB on: LR4 low path, kept clean] ──────┐ │
//!          └─> [LR4 high path / whole signal] ─> drive  │ │
//!               ─> 4x oversample ─> curve(x+bias) ─> DC │ │
//!               ─> auto-comp ──────────────────────────(+)│
//!                                     ─> output gain ─> mix out
//!
//! Eleven shapes span the continuum, mild to wild: Tape, Tube, Cubic,
//! Diode, Clip, Rectify, Fold, Sine Fold, Wrap, Crush, Decimate. The
//! first nine are static curves run inside the oversampler; Crush and
//! Decimate are lo-fi digital modes that run at the raw rate (aliasing is
//! the instrument there) with DRIVE as intensity — bits / hold length —
//! instead of level. Bias pushes any curve asymmetric; the sub split
//! keeps everything below the crossover un-shaped so bass stays clean
//! while the top burns. THE SHAPE LIST IS FINAL: the stepped param's
//! saved values re-map if the count ever changes.
//!
//! Oversampling is 4x with 8th-order Butterworth IIR guard filters on the
//! way up and down — minimal phase, no latency to report, honest-but-not-
//! mastering-grade alias rejection (the last dB of it would need FIR and
//! host latency plumbing).

mod face;

use lantern_vst3::plugin::{
    Dsp, EditorFactory, MeterStore, ParamDef, ParamValues, PluginInfo,
};
use std::f32::consts::TAU;

pub use face::{background, preview_face};

// Parameter indices (== ids; forever).
pub const P_DRIVE: usize = 0;
pub const P_SHAPE: usize = 1;
pub const P_BIAS: usize = 2;
pub const P_SUB: usize = 3;
pub const P_SPLIT: usize = 4;
pub const P_MIX: usize = 5;
pub const P_OUT: usize = 6;
pub const P_AUTO: usize = 7;

/// Meter slots (pub so the preview harness can stage demo values).
pub const M_LEVEL_L: usize = 0;
pub const M_LEVEL_R: usize = 1;
pub const M_PEAK_L: usize = 2;
pub const M_PEAK_R: usize = 3;
pub const M_RMS_L: usize = 4;
pub const M_RMS_R: usize = 5;

// ============================================================================
// Parameter mappings
// ============================================================================

fn drive_plain(n: f64) -> f64 {
    n * 36.0
}
fn drive_norm(p: f64) -> f64 {
    p / 36.0
}
fn drive_fmt(n: f64) -> String {
    format!("{:.1}", drive_plain(n))
}

fn shape_fmt(n: f64) -> String {
    [
        "Tape", "Tube", "Cubic", "Diode", "Clip", "Rectify", "Fold", "Sine Fold", "Wrap",
        "Crush", "Decimate",
    ][((n * 10.0).round() as usize).min(10)]
    .to_string()
}

fn bias_plain(n: f64) -> f64 {
    n * 200.0 - 100.0
}
fn bias_norm(p: f64) -> f64 {
    (p + 100.0) / 200.0
}
fn bias_fmt(n: f64) -> String {
    let p = bias_plain(n).round();
    if p == 0.0 {
        "0".to_string()
    } else {
        format!("{p:+.0}")
    }
}

fn split_plain(n: f64) -> f64 {
    40.0 * 10f64.powf(n) // 40 .. 400 Hz, log
}
fn split_norm(p: f64) -> f64 {
    (p.max(40.0) / 40.0).log10()
}
fn split_fmt(n: f64) -> String {
    format!("{:.0}", split_plain(n))
}

fn pct_plain(n: f64) -> f64 {
    n * 100.0
}
fn pct_norm(p: f64) -> f64 {
    p / 100.0
}
fn pct_fmt(n: f64) -> String {
    format!("{:.0}", pct_plain(n))
}

fn out_plain(n: f64) -> f64 {
    n * 48.0 - 24.0
}
fn out_norm(p: f64) -> f64 {
    (p + 24.0) / 48.0
}
fn out_fmt(n: f64) -> String {
    format!("{:+.1}", out_plain(n))
}

fn on_fmt(n: f64) -> String {
    if n >= 0.5 { "On" } else { "Off" }.to_string()
}

macro_rules! p {
    ($id:expr, $title:expr, $units:expr, $def:expr, $steps:expr, $tp:expr, $fp:expr, $fmt:expr) => {
        ParamDef {
            id: $id,
            title: $title,
            short_title: $title,
            units: $units,
            default_normalized: $def,
            step_count: $steps,
            can_automate: true,
            to_plain: $tp,
            from_plain: $fp,
            format: $fmt,
        }
    };
}

// ============================================================================
// The curves (shared with the face: drawn = heard)
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// Soft tanh — low-order odd harmonics, the warm end.
    Tape,
    /// Asymmetric tanh (the negative half saturates 1.5x sooner) — even
    /// harmonics, the "one side of the wave clips first" tube thing.
    Tube,
    /// x - x³/3, ceiling-normalized — only a gentle 3rd until pushed; the
    /// barely-there option.
    Cubic,
    /// Exponential knee — bends from the first millivolt like clipping
    /// diodes; always slightly dirty, pedal flavor.
    Diode,
    /// Hard clip at ±1 — high-order odd harmonics, bright and mean.
    Clip,
    /// Half-wave rectify into a soft ceiling — strong even harmonics,
    /// octave-up shimmer, broken-amp energy.
    Rectify,
    /// Triangle wavefolder — past ±1 the wave reflects instead of
    /// flattening; harsher-than-clip inharmonic-sounding fizz.
    Fold,
    /// West Coast smooth fold (sine of the driven signal) — liquid,
    /// vocal, the musical extreme.
    SineFold,
    /// Modulo wraparound — the wave jumps from ceiling to floor; digital
    /// chaos past the edge.
    Wrap,
    /// Bit-depth reduction, raw rate. DRIVE = bits (64 levels down to 2).
    Crush,
    /// Sample-and-hold rate reduction, raw rate. DRIVE = hold (1x..64x).
    Decimate,
}

impl Shape {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Tape,
            1 => Self::Tube,
            2 => Self::Cubic,
            3 => Self::Diode,
            4 => Self::Clip,
            5 => Self::Rectify,
            6 => Self::Fold,
            7 => Self::SineFold,
            8 => Self::Wrap,
            9 => Self::Crush,
            _ => Self::Decimate,
        }
    }

    /// Lo-fi digital modes: run at the raw rate (no oversampling — the
    /// aliasing is the instrument) and read DRIVE as intensity, not level.
    pub fn digital(self) -> bool {
        matches!(self, Self::Crush | Self::Decimate)
    }
}

/// Crush's quantizer levels for a 0..1 drive: 64 (subtle) down to 2 (1-bit
/// brutality). Shared with the face so the staircase drawn is the one heard.
pub fn crush_levels(drive01: f32) -> f32 {
    2f32.powf(1.0 + (1.0 - drive01.clamp(0.0, 1.0)) * 5.0)
}

/// The raw static transfer curve. Crush/Decimate are time/intensity modes
/// handled in `Channel::run`, not static curves — here they pass through.
pub fn wave(shape: Shape, x: f32) -> f32 {
    match shape {
        Shape::Tape => x.tanh(),
        Shape::Tube => {
            if x >= 0.0 {
                x.tanh()
            } else {
                (1.5 * x).tanh() / 1.5
            }
        }
        Shape::Cubic => {
            let c = x.clamp(-1.0, 1.0);
            1.5 * (c - c * c * c / 3.0)
        }
        Shape::Diode => x.signum() * (1.0 - (-x.abs()).exp()),
        Shape::Clip => x.clamp(-1.0, 1.0),
        Shape::Rectify => (2.0 * x.max(0.0)).tanh(),
        Shape::Fold => 1.0 - (4.0 * ((x + 1.0) * 0.25).rem_euclid(1.0) - 2.0).abs(),
        Shape::SineFold => (std::f32::consts::FRAC_PI_2 * x).sin(),
        Shape::Wrap => (x + 1.0).rem_euclid(2.0) - 1.0,
        Shape::Crush | Shape::Decimate => x,
    }
}

/// The curve as the signal sees it: bias pushed in before the curve, the
/// curve's response to bias alone subtracted so silence stays silence.
pub fn shaped(shape: Shape, x: f32, bias: f32) -> f32 {
    wave(shape, x + bias) - wave(shape, bias)
}

// ============================================================================
// Building blocks
// ============================================================================

/// RBJ biquad, lowpass/highpass.
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
    fn set(&mut self, freq: f32, q: f32, sample_rate: f32, highpass: bool) {
        let w0 = TAU * (freq / sample_rate).min(0.49);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        if highpass {
            self.b0 = (1.0 + cos) / (2.0 * a0);
            self.b1 = -(1.0 + cos) / a0;
        } else {
            self.b0 = (1.0 - cos) / (2.0 * a0);
            self.b1 = (1.0 - cos) / a0;
        }
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

const OS: usize = 4;
/// 8th-order Butterworth as four cascaded biquads.
const BW8_Q: [f32; 4] = [0.5098, 0.6013, 0.9000, 2.5629];

/// 4x oversampler: zero-stuff -> guard lowpass -> shape -> guard lowpass
/// -> decimate, all IIR so the latency is phase, not samples.
#[derive(Clone, Copy, Default)]
struct Oversampler {
    up: [Biquad; 4],
    down: [Biquad; 4],
}

impl Oversampler {
    fn setup(&mut self, sample_rate: f32) {
        // Guard cutoff just under the ORIGINAL Nyquist (0.84x of it), so
        // the audible band passes untouched; filters run at the 4x rate.
        let fc = 0.42 * sample_rate;
        for (f, q) in self.up.iter_mut().zip(BW8_Q) {
            f.set(fc, q, sample_rate * OS as f32, false);
        }
        for (f, q) in self.down.iter_mut().zip(BW8_Q) {
            f.set(fc, q, sample_rate * OS as f32, false);
        }
    }

    fn reset(&mut self) {
        for f in &mut self.up {
            f.reset();
        }
        for f in &mut self.down {
            f.reset();
        }
    }

    #[inline]
    fn process(&mut self, x: f32, mut curve: impl FnMut(f32) -> f32) -> f32 {
        let mut out = 0.0;
        for k in 0..OS {
            // Zero-stuff (x OS restores the passband gain).
            let mut v = if k == 0 { x * OS as f32 } else { 0.0 };
            for f in &mut self.up {
                v = f.process(v);
            }
            v = curve(v);
            for f in &mut self.down {
                v = f.process(v);
            }
            out = v;
        }
        out
    }
}

/// One channel's stateful path.
#[derive(Clone, Copy, Default)]
struct Channel {
    /// LR4 crossover: two cascaded Butterworth 2nd-orders per path.
    lo: [Biquad; 2],
    hi: [Biquad; 2],
    os: Oversampler,
    // DC blocker (~8 Hz one-pole HP): bias and asymmetric curves shift
    // the average; fold can park it way off center.
    dc_x: f32,
    dc_y: f32,
    // Decimate's sample-and-hold: the held value and a fractional clock.
    dec_held: f32,
    dec_phase: f32,
}

impl Channel {
    fn set_split(&mut self, freq: f32, sample_rate: f32) {
        for f in &mut self.lo {
            f.set(freq, std::f32::consts::FRAC_1_SQRT_2, sample_rate, false);
        }
        for f in &mut self.hi {
            f.set(freq, std::f32::consts::FRAC_1_SQRT_2, sample_rate, true);
        }
    }

    fn reset(&mut self) {
        for f in &mut self.lo {
            f.reset();
        }
        for f in &mut self.hi {
            f.reset();
        }
        self.os.reset();
        self.dc_x = 0.0;
        self.dc_y = 0.0;
        self.dec_held = 0.0;
        self.dec_phase = 1.0;
    }

    /// The wet path for one sample: split (maybe), drive, shape at 4x —
    /// or lo-fi at the raw rate — DC-block, compensate, and glue the
    /// clean low back on.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        x: f32,
        split_on: bool,
        shape: Shape,
        gain: f32,
        drive01: f32,
        bias: f32,
        comp: f32,
        dc_r: f32,
    ) -> f32 {
        let (low, high) = if split_on {
            let mut lo = x;
            for f in &mut self.lo {
                lo = f.process(lo);
            }
            let mut hi = x;
            for f in &mut self.hi {
                hi = f.process(hi);
            }
            (lo, hi)
        } else {
            (0.0, x)
        };
        let driven = if shape.digital() {
            // Lo-fi runs raw — aliasing is the instrument, oversampling
            // would only sand off the point. DRIVE is intensity here.
            match shape {
                Shape::Crush => {
                    let l = crush_levels(drive01);
                    (high.clamp(-1.0, 1.0) * l).round() / l
                }
                _ => {
                    self.dec_phase += 1.0 / (1.0 + drive01 * 63.0);
                    if self.dec_phase >= 1.0 {
                        self.dec_phase -= 1.0;
                        self.dec_held = high;
                    }
                    self.dec_held
                }
            }
        } else {
            self.os.process(high * gain, |v| shaped(shape, v, bias))
        };
        let blocked = driven - self.dc_x + dc_r * self.dc_y;
        self.dc_x = driven;
        self.dc_y = blocked;
        low + blocked * comp
    }
}

// ============================================================================
// The DSP
// ============================================================================

pub struct WaveshaperDsp {
    sample_rate: f32,
    channels: [Channel; 2],
    split_freq: f32,
    split_on_prev: bool,
    dc_r: f32,
    // One-pole smoothed controls (zipper-noise control).
    sm_gain: f32,
    sm_bias: f32,
    sm_comp: f32,
    sm_mix: f32,
    sm_out: f32,
    smooth_coef: f32,
    // Output metering (family standard).
    env_l: f32,
    env_r: f32,
    env_decay: f32,
    ms_l: f32,
    ms_r: f32,
    ms_coef: f32,
}

impl Dsp for WaveshaperDsp {
    const INFO: PluginInfo = PluginInfo {
        name: "Lantern Waveshaper",
        vendor: "Alva",
        version: "0.1.0",
        url: "https://github.com/",
        email: "noreply@example.com",
        class_id: *b"LanternWaveshape",
        subcategories: "Fx|Distortion",
    };

    #[rustfmt::skip]
    const PARAMS: &'static [ParamDef] = &[
        p!(0, "Drive",     "dB", 0.0,       0, Some(drive_plain), Some(drive_norm), Some(drive_fmt)),
        p!(1, "Shape",     "",   0.0,      10, None, None, Some(shape_fmt)),
        p!(2, "Bias",      "%",  0.5,       0, Some(bias_plain), Some(bias_norm), Some(bias_fmt)),
        p!(3, "Sub Split", "",   0.0,       1, None, None, Some(on_fmt)),
        p!(4, "Split",     "Hz", 0.4771,    0, Some(split_plain), Some(split_norm), Some(split_fmt)),
        p!(5, "Mix",       "%",  1.0,       0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(6, "Output",    "dB", 0.5,       0, Some(out_plain), Some(out_norm), Some(out_fmt)),
        // Off by default, no face control — Alva's rule: never auto
        // anything by default. Reachable via host automation only.
        p!(7, "Auto Gain", "",   0.0,       1, None, None, Some(on_fmt)),
    ];

    const METERS: usize = 6;
    const EDITOR: Option<EditorFactory> = Some(face::make_editor);

    fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            channels: [Channel::default(); 2],
            split_freq: -1.0,
            split_on_prev: false,
            dc_r: 0.999,
            sm_gain: 2.0,
            sm_bias: 0.0,
            sm_comp: 1.0,
            sm_mix: 1.0,
            sm_out: 1.0,
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
        let sr = sample_rate as f32;
        self.sample_rate = sr;
        self.split_freq = -1.0; // force crossover recompute
        for c in &mut self.channels {
            c.os.setup(sr);
        }
        self.dc_r = 1.0 - TAU * 8.0 / sr;
        // ~20 ms control smoothing; family meter ballistics.
        self.smooth_coef = 1.0 - (-1.0 / (sr * 0.02)).exp();
        self.env_decay = (0.1f32.ln() / (1.5 * sr)).exp();
        self.ms_coef = 1.0 - (-1.0 / (sr * 0.3)).exp();
    }

    fn reset(&mut self) {
        for c in &mut self.channels {
            c.reset();
        }
        self.env_l = 0.0;
        self.env_r = 0.0;
        self.ms_l = 0.0;
        self.ms_r = 0.0;
    }

    fn process(&mut self, buffers: &mut [&mut [f32]], params: &ParamValues, meters: &MeterStore) {
        let shape = Shape::from_index((params.normalized(P_SHAPE) * 10.0).round() as usize);
        let gain_t = 10f32.powf(params.plain(P_DRIVE) as f32 / 20.0);
        let bias_t = params.plain(P_BIAS) as f32 / 100.0 * 0.75;
        let mix_t = params.plain(P_MIX) as f32 / 100.0;
        let out_t = 10f32.powf(params.plain(P_OUT) as f32 / 20.0);
        let split_on = params.normalized(P_SUB) >= 0.5;
        let auto_on = params.normalized(P_AUTO) >= 0.5;

        let split_f = params.plain(P_SPLIT) as f32;
        if split_on && split_f != self.split_freq {
            for c in &mut self.channels {
                c.set_split(split_f, self.sample_rate);
            }
            self.split_freq = split_f;
        }
        if split_on != self.split_on_prev {
            // Fresh crossover state on toggle; stale filters would thump.
            for c in &mut self.channels {
                for f in c.lo.iter_mut().chain(c.hi.iter_mut()) {
                    f.reset();
                }
            }
            self.split_on_prev = split_on;
        }

        // Auto gain: match the RMS of a -6 dBFS sine through the curve to
        // its clean self, once per block (then smoothed like everything).
        // The lo-fi modes hold level by construction — no comp there.
        let comp_t = if auto_on && !shape.digital() {
            let mut acc = 0.0;
            for i in 0..32 {
                let s = 0.5 * (TAU * i as f32 / 32.0).sin();
                let y = shaped(shape, s * gain_t, bias_t);
                acc += y * y;
            }
            let rms = (acc / 32.0).sqrt();
            (0.353_55 / rms.max(1e-4)).clamp(0.125, 8.0)
        } else {
            1.0
        };

        let (first, rest) = buffers.split_at_mut(1);
        let ch_l = &mut *first[0];
        let mut ch_r = rest.first_mut();
        let num_samples = ch_l.len();

        let mut block_peak_l = 0.0f32;
        let mut block_peak_r = 0.0f32;

        for i in 0..num_samples {
            self.sm_gain += (gain_t - self.sm_gain) * self.smooth_coef;
            self.sm_bias += (bias_t - self.sm_bias) * self.smooth_coef;
            self.sm_comp += (comp_t - self.sm_comp) * self.smooth_coef;
            self.sm_mix += (mix_t - self.sm_mix) * self.smooth_coef;
            self.sm_out += (out_t - self.sm_out) * self.smooth_coef;

            let in_l = ch_l[i];
            let in_r = ch_r.as_ref().map(|c| c[i]).unwrap_or(in_l);

            // The lo-fi modes read DRIVE as 0..1 intensity (via the same
            // smoothed gain, so sweeps stay zipper-free).
            let drive01 = (20.0 * self.sm_gain.max(1e-6).log10() / 36.0).clamp(0.0, 1.0);

            let wet_l = self.channels[0].run(
                in_l, split_on, shape, self.sm_gain, drive01, self.sm_bias, self.sm_comp,
                self.dc_r,
            ) * self.sm_out;
            let wet_r = self.channels[1].run(
                in_r, split_on, shape, self.sm_gain, drive01, self.sm_bias, self.sm_comp,
                self.dc_r,
            ) * self.sm_out;

            let out_l = in_l + (wet_l - in_l) * self.sm_mix;
            let out_r = in_r + (wet_r - in_r) * self.sm_mix;

            ch_l[i] = out_l;
            if let Some(ch_r) = ch_r.as_mut() {
                ch_r[i] = out_r;
            }

            let (al, ar) = (out_l.abs(), out_r.abs());
            self.env_l = if al > self.env_l { al } else { self.env_l * self.env_decay };
            self.env_r = if ar > self.env_r { ar } else { self.env_r * self.env_decay };
            block_peak_l = block_peak_l.max(al);
            block_peak_r = block_peak_r.max(ar);
            self.ms_l += (out_l * out_l - self.ms_l) * self.ms_coef;
            self.ms_r += (out_r * out_r - self.ms_r) * self.ms_coef;
        }

        meters.set(M_LEVEL_L, self.env_l as f64);
        meters.set(M_LEVEL_R, self.env_r as f64);
        meters.set(M_PEAK_L, meters.get(M_PEAK_L).max(block_peak_l as f64));
        meters.set(M_PEAK_R, meters.get(M_PEAK_R).max(block_peak_r as f64));
        meters.set(M_RMS_L, self.ms_l.sqrt() as f64);
        meters.set(M_RMS_R, self.ms_r.sqrt() as f64);
    }
}

lantern_vst3::export_plugin!(WaveshaperDsp);

#[cfg(test)]
mod tests {
    use super::*;
    use lantern_vst3::plugin::ParamStore;

    fn run_blocks(dsp: &mut WaveshaperDsp, store: &ParamStore, blocks: usize, f: f32) -> Vec<f32> {
        let params = ParamValues::preview(store, WaveshaperDsp::PARAMS);
        let meters = MeterStore::new(WaveshaperDsp::METERS);
        let mut out = Vec::new();
        let mut phase = 0.0f32;
        for _ in 0..blocks {
            let mut l = [0.0f32; 256];
            let mut r = [0.0f32; 256];
            for i in 0..256 {
                let s = (TAU * phase).sin() * 0.5;
                phase = (phase + f / 48_000.0).fract();
                l[i] = s;
                r[i] = s;
            }
            {
                let mut bufs: [&mut [f32]; 2] = [&mut l, &mut r];
                dsp.process(&mut bufs, &params, &meters);
            }
            out.extend_from_slice(&l);
        }
        out
    }

    /// Every shape at max drive and full bias stays finite and bounded.
    #[test]
    fn bounded_at_full_send() {
        for shape in 0..11 {
            let store = ParamStore::new(WaveshaperDsp::PARAMS);
            store.set(P_DRIVE, 1.0);
            store.set(P_SHAPE, shape as f64 / 10.0);
            store.set(P_BIAS, 1.0);
            store.set(P_SUB, 1.0);
            let mut dsp = WaveshaperDsp::new();
            dsp.setup(48_000.0, 256);
            let out = run_blocks(&mut dsp, &store, 40, 220.0);
            for (i, s) in out.iter().enumerate() {
                assert!(s.is_finite(), "shape {shape}: non-finite at {i}");
                assert!(s.abs() < 16.0, "shape {shape}: blew up at {i}: {s}");
            }
        }
    }

    /// Mix 0 = the plugin gets out of the way (after smoothing settles).
    #[test]
    fn mix_zero_is_transparent() {
        let store = ParamStore::new(WaveshaperDsp::PARAMS);
        store.set(P_DRIVE, 1.0);
        store.set(P_SHAPE, 0.8); // Wrap, the wildest static curve
        store.set(P_MIX, 0.0);
        let mut dsp = WaveshaperDsp::new();
        dsp.setup(48_000.0, 256);
        let out = run_blocks(&mut dsp, &store, 40, 220.0);
        // Compare the tail (smoothers settled) against the same sine.
        let mut phase = 0.0f32;
        let reference: Vec<f32> = (0..out.len())
            .map(|_| {
                let s = (TAU * phase).sin() * 0.5;
                phase = (phase + 220.0 / 48_000.0).fract();
                s
            })
            .collect();
        for i in out.len() - 2048..out.len() {
            assert!(
                (out[i] - reference[i]).abs() < 1e-3,
                "not transparent at {i}: {} vs {}",
                out[i],
                reference[i]
            );
        }
    }

    /// With the split on, a sub-band sine survives a hard clipping stage
    /// nearly untouched while a midrange sine gets visibly squashed.
    #[test]
    fn sub_split_keeps_bass_clean() {
        let rms = |v: &[f32]| (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt();
        let peak = |v: &[f32]| v.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        let mut stats = [(0.0f32, 0.0f32); 2];
        for (which, freq) in [(0usize, 50.0f32), (1, 1000.0)] {
            let store = ParamStore::new(WaveshaperDsp::PARAMS);
            store.set(P_DRIVE, 1.0 / 3.0); // 12 dB -> gain 4
            store.set(P_SHAPE, 0.4); // Clip
            store.set(P_SUB, 1.0); // split at the default 120 Hz
            store.set(P_AUTO, 0.0);
            let mut dsp = WaveshaperDsp::new();
            dsp.setup(48_000.0, 256);
            let out = run_blocks(&mut dsp, &store, 40, freq);
            let tail = &out[out.len() - 4096..];
            stats[which] = (rms(tail), peak(tail));
        }
        // Sub: the clean 0.5 sine (RMS 0.354), give or take crossover leak.
        assert!(
            (0.28..0.45).contains(&stats[0].0) && stats[0].1 < 0.65,
            "sub not clean: rms {} peak {}",
            stats[0].0,
            stats[0].1
        );
        // Mids: 2x over the ceiling, clipped — RMS jumps toward a square's.
        assert!(stats[1].0 > 0.75, "mids not clipping: rms {}", stats[1].0);
    }
}

