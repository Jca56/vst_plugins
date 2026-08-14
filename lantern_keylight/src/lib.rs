//! Lantern Keylight — tuner, pitch shifter, utility, and a scale cheat
//! sheet in one face. Grew out of Lantern Gain: same zero-dependency
//! plumbing, now with a two-tap granular shifter, a YIN tuner listening to
//! the output, and a piano-strip scale guide for tuning samples into key.
//!
//! Signal flow: pitch shift -> mono sum -> balance -> gain -> tuner tap.

mod face;
mod music;
mod pitch;
mod tuner;

pub use face::preview_face;

use lantern_vst3::plugin::{
    Dsp, EditorFactory, MeterStore, ParamDef, ParamValues, PluginInfo,
};

// Parameter indices (order in PARAMS; IDs match, and are forever).
pub(crate) const P_SHIFT: usize = 0;
pub(crate) const P_FINE: usize = 1;
pub(crate) const P_GAIN: usize = 2;
pub(crate) const P_BALANCE: usize = 3;
pub(crate) const P_MONO: usize = 4;
pub(crate) const P_ROOT: usize = 5;
pub(crate) const P_SCALE: usize = 6;
pub(crate) const P_BASS_MONO: usize = 7;
pub(crate) const P_BASS_FREQ: usize = 8;

/// Meter slots (audio thread -> editor; pub so the preview harness can
/// stage demo values).
pub const M_FREQ: usize = 0;
pub const M_CLARITY: usize = 1;
/// Output level envelope (instant attack, ~20 dB / 1.5 s fall).
pub const M_LEVEL_L: usize = 2;
pub const M_LEVEL_R: usize = 3;
/// Held peak, max-since-reset. The editor zeroes these on click, so the
/// audio thread max-accumulates into the slot instead of keeping a copy.
pub const M_PEAK_L: usize = 4;
pub const M_PEAK_R: usize = 5;
/// RMS (~300 ms exponentially-weighted window): perceived-loudness core.
pub const M_RMS_L: usize = 6;
pub const M_RMS_R: usize = 7;

// ============================================================================
// Parameter mappings (normalized 0..1 <-> display values)
// ============================================================================

fn shift_plain(n: f64) -> f64 {
    (n * 24.0).round() - 12.0
}
fn shift_norm(p: f64) -> f64 {
    (p + 12.0) / 24.0
}
fn shift_fmt(n: f64) -> String {
    let p = shift_plain(n);
    if p == 0.0 {
        "0".to_string()
    } else {
        format!("{p:+.0}")
    }
}

fn fine_plain(n: f64) -> f64 {
    n * 100.0 - 50.0
}
fn fine_norm(p: f64) -> f64 {
    (p + 50.0) / 100.0
}
fn fine_fmt(n: f64) -> String {
    let p = fine_plain(n);
    if p.round() == 0.0 {
        "0".to_string()
    } else {
        format!("{p:+.0}")
    }
}

fn gain_plain(n: f64) -> f64 {
    n * 48.0 - 24.0
}
fn gain_norm(p: f64) -> f64 {
    (p + 24.0) / 48.0
}
fn gain_fmt(n: f64) -> String {
    format!("{:+.1}", gain_plain(n))
}

fn bal_plain(n: f64) -> f64 {
    n * 100.0 - 50.0
}
fn bal_norm(p: f64) -> f64 {
    (p + 50.0) / 100.0
}
fn bal_fmt(n: f64) -> String {
    let p = bal_plain(n);
    if p.abs() < 0.5 {
        "C".to_string()
    } else if p < 0.0 {
        format!("{:.0}L", -p)
    } else {
        format!("{p:.0}R")
    }
}

fn mono_fmt(n: f64) -> String {
    if n >= 0.5 { "On" } else { "Off" }.to_string()
}

fn root_plain(n: f64) -> f64 {
    (n * 11.0).round()
}
fn root_norm(p: f64) -> f64 {
    p / 11.0
}
fn root_fmt(n: f64) -> String {
    music::NOTE_NAMES[(root_plain(n) as usize) % 12].to_string()
}

fn scale_steps() -> f64 {
    (music::SCALES.len() - 1) as f64
}
fn scale_plain(n: f64) -> f64 {
    (n * scale_steps()).round()
}
fn scale_norm(p: f64) -> f64 {
    p / scale_steps()
}
fn scale_fmt(n: f64) -> String {
    music::SCALES[(scale_plain(n) as usize).min(music::SCALES.len() - 1)]
        .name
        .to_string()
}

fn bfreq_plain(n: f64) -> f64 {
    50.0 * 10f64.powf(n) // 50 .. 500 Hz, log
}
fn bfreq_norm(p: f64) -> f64 {
    (p.max(50.0) / 50.0).log10()
}
fn bfreq_fmt(n: f64) -> String {
    format!("{:.0}", bfreq_plain(n))
}

/// Balance law: unity at center, opposite side fades on a quarter cosine.
fn balance_gains(pan: f32) -> (f32, f32) {
    let a = (pan.abs().min(1.0) * std::f32::consts::FRAC_PI_2).cos();
    if pan > 0.0 {
        (a, 1.0)
    } else {
        (1.0, a)
    }
}

// ============================================================================
// The DSP
// ============================================================================

pub struct KeylightDsp {
    shifter: pitch::PitchShifter,
    /// Listens to the output; drives the tuner display.
    detector: tuner::PitchDetector,
    /// Listens to the input; period-locks the shifter's grain window.
    in_detector: tuner::PitchDetector,
    sample_rate: f32,
    // One-pole smoothed control values (zipper-noise control).
    sm_shift: f32,
    sm_dry: f32,
    sm_mono: f32,
    sm_bass_mono: f32,
    /// Cascaded one-pole lowpass states (12 dB/oct bass-mono crossover).
    bm_lp: [f32; 4],
    sm_gl: f32,
    sm_gr: f32,
    sm_gain: f32,
    coef_fast: f32,
    coef_mid: f32,
    // Output level metering.
    env_l: f32,
    env_r: f32,
    env_decay: f32,
    // Running mean square (RMS before the square root).
    ms_l: f32,
    ms_r: f32,
    ms_coef: f32,
}

impl Dsp for KeylightDsp {
    const INFO: PluginInfo = PluginInfo {
        name: "Lantern Keylight",
        vendor: "Alva",
        version: "0.1.0",
        url: "https://github.com/",
        email: "noreply@example.com",
        class_id: *b"LanternKeylight!",
        subcategories: "Fx|Tools",
    };

    const PARAMS: &'static [ParamDef] = &[
        ParamDef {
            id: 0,
            title: "Shift",
            short_title: "Shift",
            units: "st",
            default_normalized: 0.5,
            step_count: 24, // -12 .. +12 semitones
            can_automate: true,
            to_plain: Some(shift_plain),
            from_plain: Some(shift_norm),
            format: Some(shift_fmt),
        },
        ParamDef {
            id: 1,
            title: "Fine",
            short_title: "Fine",
            units: "ct",
            default_normalized: 0.5,
            step_count: 0,
            can_automate: true,
            to_plain: Some(fine_plain),
            from_plain: Some(fine_norm),
            format: Some(fine_fmt),
        },
        ParamDef {
            id: 2,
            title: "Gain",
            short_title: "Gain",
            units: "dB",
            default_normalized: 0.5,
            step_count: 0,
            can_automate: true,
            to_plain: Some(gain_plain),
            from_plain: Some(gain_norm),
            format: Some(gain_fmt),
        },
        ParamDef {
            id: 3,
            title: "Balance",
            short_title: "Bal",
            units: "",
            default_normalized: 0.5,
            step_count: 0,
            can_automate: true,
            to_plain: Some(bal_plain),
            from_plain: Some(bal_norm),
            format: Some(bal_fmt),
        },
        ParamDef {
            id: 4,
            title: "Mono",
            short_title: "Mono",
            units: "",
            default_normalized: 0.0,
            step_count: 1,
            can_automate: true,
            to_plain: None,
            from_plain: None,
            format: Some(mono_fmt),
        },
        ParamDef {
            id: 5,
            title: "Root",
            short_title: "Root",
            units: "",
            default_normalized: 0.0, // C
            step_count: 11,
            can_automate: false,
            to_plain: Some(root_plain),
            from_plain: Some(root_norm),
            format: Some(root_fmt),
        },
        ParamDef {
            id: 6,
            title: "Scale",
            short_title: "Scale",
            units: "",
            default_normalized: 1.0 / 11.0, // Natural Minor
            step_count: 11,
            can_automate: false,
            to_plain: Some(scale_plain),
            from_plain: Some(scale_norm),
            format: Some(scale_fmt),
        },
        ParamDef {
            id: 7,
            title: "Bass Mono",
            short_title: "BassM",
            units: "",
            default_normalized: 0.0,
            step_count: 1,
            can_automate: true,
            to_plain: None,
            from_plain: None,
            format: Some(mono_fmt),
        },
        ParamDef {
            id: 8,
            title: "Bass Freq",
            short_title: "BFreq",
            units: "Hz",
            default_normalized: 0.3802, // ~120 Hz
            step_count: 0,
            can_automate: true,
            to_plain: Some(bfreq_plain),
            from_plain: Some(bfreq_norm),
            format: Some(bfreq_fmt),
        },
    ];

    const METERS: usize = 8;
    const EDITOR: Option<EditorFactory> = Some(face::make_editor);

    fn new() -> Self {
        Self {
            shifter: pitch::PitchShifter::new(),
            detector: tuner::PitchDetector::new(),
            in_detector: tuner::PitchDetector::new(),
            sample_rate: 48_000.0,
            sm_shift: 0.0,
            sm_dry: 1.0,
            sm_mono: 0.0,
            sm_bass_mono: 0.0,
            bm_lp: [0.0; 4],
            sm_gl: 1.0,
            sm_gr: 1.0,
            sm_gain: 1.0,
            coef_fast: 0.0,
            coef_mid: 0.0,
            env_l: 0.0,
            env_r: 0.0,
            env_decay: 1.0,
            ms_l: 0.0,
            ms_r: 0.0,
            ms_coef: 0.0,
        }
    }

    fn setup(&mut self, sample_rate: f64, _max_block_size: usize) {
        self.shifter.setup(sample_rate);
        self.detector.setup(sample_rate);
        self.in_detector.setup(sample_rate);
        let sr = sample_rate as f32;
        self.sample_rate = sr;
        // ~5 ms for gain/balance, ~10 ms for shift glide and dry/wet fades.
        self.coef_fast = 1.0 - (-1.0 / (sr * 0.005)).exp();
        self.coef_mid = 1.0 - (-1.0 / (sr * 0.010)).exp();
        // Meter fall: -20 dB per 1.5 s (instant attack in the loop).
        self.env_decay = (0.1f32.ln() / (1.5 * sr)).exp();
        // ~300 ms RMS integration window.
        self.ms_coef = 1.0 - (-1.0 / (sr * 0.3)).exp();
    }

    fn reset(&mut self) {
        self.shifter.reset();
        self.detector.reset();
        self.in_detector.reset();
        self.sm_shift = 0.0;
        self.sm_dry = 1.0;
        self.sm_mono = 0.0;
        self.sm_bass_mono = 0.0;
        self.bm_lp = [0.0; 4];
        self.sm_gl = 1.0;
        self.sm_gr = 1.0;
        self.sm_gain = 1.0;
        self.env_l = 0.0;
        self.env_r = 0.0;
        self.ms_l = 0.0;
        self.ms_r = 0.0;
    }

    fn process(&mut self, buffers: &mut [&mut [f32]], params: &ParamValues, meters: &MeterStore) {
        // Per-block targets.
        let shift_target = params.plain(P_SHIFT) as f32 + params.plain(P_FINE) as f32 / 100.0;
        let dry_target = if shift_target.abs() < 1e-3 { 1.0 } else { 0.0 };
        let mono_target = if params.normalized(P_MONO) >= 0.5 { 1.0 } else { 0.0 };
        let bass_mono_target = if params.normalized(P_BASS_MONO) >= 0.5 { 1.0 } else { 0.0 };
        let bm_coef = 1.0
            - (-std::f32::consts::TAU * params.plain(P_BASS_FREQ) as f32 / self.sample_rate).exp();
        let (gl_target, gr_target) = balance_gains(params.plain(P_BALANCE) as f32 / 50.0);
        let gain_target = 10f32.powf(params.plain(P_GAIN) as f32 / 20.0);

        // Period-lock the grain window to the input's pitch (last block's
        // detection; it refreshes every ~43 ms).
        let in_f = self.in_detector.freq();
        self.shifter.set_period(
            (self.in_detector.clarity() > 0.6 && (24.0..=2000.0).contains(&in_f))
                .then(|| self.sample_rate / in_f),
        );

        let (first, rest) = buffers.split_at_mut(1);
        let ch_l = &mut *first[0];
        let mut ch_r = rest.first_mut();
        let num_samples = ch_l.len();

        let mut block_peak_l = 0.0f32;
        let mut block_peak_r = 0.0f32;

        for i in 0..num_samples {
            self.sm_shift += (shift_target - self.sm_shift) * self.coef_mid;
            self.sm_dry += (dry_target - self.sm_dry) * self.coef_mid;
            self.sm_mono += (mono_target - self.sm_mono) * self.coef_mid;
            self.sm_bass_mono += (bass_mono_target - self.sm_bass_mono) * self.coef_mid;
            self.sm_gl += (gl_target - self.sm_gl) * self.coef_fast;
            self.sm_gr += (gr_target - self.sm_gr) * self.coef_fast;
            self.sm_gain += (gain_target - self.sm_gain) * self.coef_fast;

            let in_l = ch_l[i];
            let in_r = ch_r.as_ref().map(|c| c[i]).unwrap_or(in_l);

            // --- Pitch: rings stay warm even in bypass ---
            self.in_detector.feed(0.5 * (in_l + in_r));
            self.shifter.push(in_l, in_r);
            let (mut l, mut r) = if self.sm_dry > 0.999 {
                (in_l, in_r)
            } else {
                let ratio = (self.sm_shift / 12.0).exp2();
                let (wl, wr) = self.shifter.taps(ratio);
                (
                    wl + (in_l - wl) * self.sm_dry,
                    wr + (in_r - wr) * self.sm_dry,
                )
            };

            // --- Mono sum (crossfaded so the button doesn't click) ---
            let mid = 0.5 * (l + r);
            l += (mid - l) * self.sm_mono;
            r += (mid - r) * self.sm_mono;

            // --- Bass mono: sum lows to center, keep highs stereo ---
            // 12 dB/oct low split per channel; subtracting the low band's
            // side signal is exactly transparent when lows are already mono.
            self.bm_lp[0] += (l - self.bm_lp[0]) * bm_coef;
            self.bm_lp[1] += (self.bm_lp[0] - self.bm_lp[1]) * bm_coef;
            self.bm_lp[2] += (r - self.bm_lp[2]) * bm_coef;
            self.bm_lp[3] += (self.bm_lp[2] - self.bm_lp[3]) * bm_coef;
            let low_side = 0.5 * (self.bm_lp[1] - self.bm_lp[3]) * self.sm_bass_mono;
            l -= low_side;
            r += low_side;

            // --- Balance + gain ---
            l *= self.sm_gl * self.sm_gain;
            r *= self.sm_gr * self.sm_gain;

            ch_l[i] = l;
            if let Some(ch_r) = ch_r.as_mut() {
                ch_r[i] = r;
            }

            // --- Tuner listens to what the world hears ---
            self.detector.feed(0.5 * (l + r));

            // --- Output metering: instant attack, exponential fall ---
            let (al, ar) = (l.abs(), r.abs());
            self.env_l = if al > self.env_l { al } else { self.env_l * self.env_decay };
            self.env_r = if ar > self.env_r { ar } else { self.env_r * self.env_decay };
            block_peak_l = block_peak_l.max(al);
            block_peak_r = block_peak_r.max(ar);
            self.ms_l += (l * l - self.ms_l) * self.ms_coef;
            self.ms_r += (r * r - self.ms_r) * self.ms_coef;
        }

        meters.set(M_FREQ, self.detector.freq() as f64);
        meters.set(M_CLARITY, self.detector.clarity() as f64);
        meters.set(M_LEVEL_L, self.env_l as f64);
        meters.set(M_LEVEL_R, self.env_r as f64);
        // Max-accumulate into the slot itself: the editor zeroes it to reset.
        meters.set(M_PEAK_L, meters.get(M_PEAK_L).max(block_peak_l as f64));
        meters.set(M_PEAK_R, meters.get(M_PEAK_R).max(block_peak_r as f64));
        meters.set(M_RMS_L, self.ms_l.sqrt() as f64);
        meters.set(M_RMS_R, self.ms_r.sqrt() as f64);
    }
}

lantern_vst3::export_plugin!(KeylightDsp);
