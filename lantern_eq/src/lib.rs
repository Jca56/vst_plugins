//! Lantern EQ — five-band parametric EQ with a spectrum analyzer.
//!
//! Band 1: high-pass or low shelf; bands 2-4: bells; band 5: low-pass or
//! high shelf. RBJ cookbook biquads in series (disabled bands crossfade
//! out), a hand-rolled 8192-point FFT feeding a 120-bin log spectrum of
//! the output through the meter store, and the analytic band responses
//! shared with the face so the drawn curves are the filters' truth.

pub mod biquad;
mod face;
mod fft;

use biquad::{BiquadState, Coeffs, Mode};
use lantern_vst3::plugin::{
    Dsp, EditorFactory, MeterStore, ParamDef, ParamValues, PluginInfo,
};

pub const BANDS: usize = 5;

// Parameter indices (== ids; 4 per band, then the two type switches).
pub fn p_enabled(band: usize) -> usize {
    band * 4
}
pub fn p_freq(band: usize) -> usize {
    band * 4 + 1
}
pub fn p_gain(band: usize) -> usize {
    band * 4 + 2
}
pub fn p_q(band: usize) -> usize {
    band * 4 + 3
}
pub const P_TYPE_LO: usize = 20;
pub const P_TYPE_HI: usize = 21;

/// Meter slots (pub so the preview harness can stage demo values).
pub const M_LEVEL_L: usize = 0;
pub const M_LEVEL_R: usize = 1;
pub const M_PEAK_L: usize = 2;
pub const M_PEAK_R: usize = 3;
pub const M_RMS_L: usize = 4;
pub const M_RMS_R: usize = 5;
/// The face needs the real rate to draw honest curves.
pub const M_SAMPLE_RATE: usize = 6;
/// 120 log-spaced spectrum bins (dB, -90..0), 20 Hz .. 20 kHz.
pub const M_SPECTRUM: usize = 7;
pub const SPEC_BINS: usize = 120;

const FFT_N: usize = 8192;
const HOP: usize = 4096;

// ============================================================================
// Parameter mappings
// ============================================================================

fn freq_plain(n: f64) -> f64 {
    20.0 * 1000f64.powf(n) // 20 Hz .. 20 kHz, log
}
fn freq_norm(p: f64) -> f64 {
    (p.max(20.0) / 20.0).log10() / 3.0
}
fn freq_fmt(n: f64) -> String {
    let f = freq_plain(n);
    if f >= 1000.0 {
        format!("{:.1}k", f / 1000.0)
    } else {
        format!("{f:.0}")
    }
}

fn gain_plain(n: f64) -> f64 {
    n * 30.0 - 15.0
}
fn gain_norm(p: f64) -> f64 {
    (p + 15.0) / 30.0
}
fn gain_fmt(n: f64) -> String {
    format!("{:+.1}", gain_plain(n))
}

fn q_plain(n: f64) -> f64 {
    0.4 * 20f64.powf(n) // 0.4 .. 8, log
}
fn q_norm(p: f64) -> f64 {
    (p.max(0.4) / 0.4).ln() / 20f64.ln()
}
fn q_fmt(n: f64) -> String {
    format!("{:.2}", q_plain(n))
}

fn on_fmt(n: f64) -> String {
    if n >= 0.5 { "On" } else { "Off" }.to_string()
}
fn type_lo_fmt(n: f64) -> String {
    if n < 0.5 { "HPF" } else { "Shelf" }.to_string()
}
fn type_hi_fmt(n: f64) -> String {
    if n < 0.5 { "LPF" } else { "Shelf" }.to_string()
}

macro_rules! band_params {
    ($idx:expr, $on:expr, $on_def:expr, $f:expr, $f_def:expr, $q_def:expr,
     $t_on:expr, $t_f:expr, $t_g:expr, $t_q:expr) => {
        [
            ParamDef {
                id: $idx * 10,
                title: $t_on,
                short_title: $t_on,
                units: "",
                default_normalized: $on_def,
                step_count: 1,
                can_automate: true,
                to_plain: None,
                from_plain: None,
                format: Some(on_fmt),
            },
            ParamDef {
                id: $idx * 10 + 1,
                title: $t_f,
                short_title: $t_f,
                units: "Hz",
                default_normalized: $f_def,
                step_count: 0,
                can_automate: true,
                to_plain: Some(freq_plain),
                from_plain: Some(freq_norm),
                format: Some(freq_fmt),
            },
            ParamDef {
                id: $idx * 10 + 2,
                title: $t_g,
                short_title: $t_g,
                units: "dB",
                default_normalized: 0.5,
                step_count: 0,
                can_automate: true,
                to_plain: Some(gain_plain),
                from_plain: Some(gain_norm),
                format: Some(gain_fmt),
            },
            ParamDef {
                id: $idx * 10 + 3,
                title: $t_q,
                short_title: $t_q,
                units: "",
                default_normalized: $q_def,
                step_count: 0,
                can_automate: true,
                to_plain: Some(q_plain),
                from_plain: Some(q_norm),
                format: Some(q_fmt),
            },
        ]
    };
}

const B1: [ParamDef; 4] = band_params!(0, 0.0, 0.0, 0.13265, 0.13265, 0.1901, "1 On", "1 Freq", "1 Gain", "1 Q");
const B2: [ParamDef; 4] = band_params!(1, 1.0, 1.0, 0.29169, 0.29169, 0.2707, "2 On", "2 Freq", "2 Gain", "2 Q");
const B3: [ParamDef; 4] = band_params!(2, 1.0, 1.0, 0.53402, 0.53402, 0.2707, "3 On", "3 Freq", "3 Gain", "3 Q");
const B4: [ParamDef; 4] = band_params!(3, 1.0, 1.0, 0.74768, 0.74768, 0.2707, "4 On", "4 Freq", "4 Gain", "4 Q");
const B5: [ParamDef; 4] = band_params!(4, 0.0, 0.0, 0.92605, 0.92605, 0.1901, "5 On", "5 Freq", "5 Gain", "5 Q");

// ============================================================================
// The DSP
// ============================================================================

struct Band {
    coeffs: Coeffs,
    state_l: BiquadState,
    state_r: BiquadState,
    /// Crossfaded enable, 0..1.
    mix: f32,
    sm_f: f32,
    sm_g: f32,
    sm_q: f32,
}

impl Band {
    fn new() -> Self {
        Self {
            coeffs: Coeffs::default(),
            state_l: BiquadState::default(),
            state_r: BiquadState::default(),
            mix: 0.0,
            sm_f: 1000.0,
            sm_g: 0.0,
            sm_q: 0.7,
        }
    }
}

pub struct EqDsp {
    sample_rate: f32,
    bands: [Band; BANDS],
    /// Snap smoothers to targets on the first block after setup/reset.
    snap: bool,
    coef_mid: f32,
    // Output metering (family standard).
    env_l: f32,
    env_r: f32,
    env_decay: f32,
    ms_l: f32,
    ms_r: f32,
    ms_coef: f32,
    // Spectrum analyzer.
    fft: Option<fft::Fft>,
    ring: Vec<f32>,
    widx: usize,
    since_hop: usize,
    hann: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// FFT-bin range (lo, hi) per display bin.
    bin_map: Vec<(u32, u32)>,
}

impl EqDsp {
    fn band_mode(&self, band: usize, params: &ParamValues) -> Mode {
        match band {
            0 => {
                if params.normalized(P_TYPE_LO) < 0.5 {
                    Mode::LowCut
                } else {
                    Mode::LowShelf
                }
            }
            4 => {
                if params.normalized(P_TYPE_HI) < 0.5 {
                    Mode::HighCut
                } else {
                    Mode::HighShelf
                }
            }
            _ => Mode::Bell,
        }
    }

    fn analyze(&mut self, meters: &MeterStore) {
        for i in 0..FFT_N {
            let s = self.ring[(self.widx + i) & (FFT_N - 1)];
            self.re[i] = s * self.hann[i];
            self.im[i] = 0.0;
        }
        if let Some(fft) = &self.fft {
            fft.forward(&mut self.re, &mut self.im);
        }
        for (i, &(lo, hi)) in self.bin_map.iter().enumerate() {
            let mut peak = 0.0f32;
            for k in lo..=hi {
                let (re, im) = (self.re[k as usize], self.im[k as usize]);
                peak = peak.max(re * re + im * im);
            }
            // Full-scale sine (Hann coherent gain 0.5) peaks at N/4.
            let mag = peak.sqrt() * 4.0 / FFT_N as f32;
            let db = (20.0 * mag.max(1e-6).log10()).clamp(-90.0, 0.0);
            meters.set(M_SPECTRUM + i, db as f64);
        }
    }
}

impl Dsp for EqDsp {
    const INFO: PluginInfo = PluginInfo {
        name: "Lantern EQ",
        vendor: "Alva",
        version: "0.1.0",
        url: "https://github.com/",
        email: "noreply@example.com",
        class_id: *b"LanternEQ5!Alva!",
        subcategories: "Fx|EQ",
    };

    const PARAMS: &'static [ParamDef] = &{
        let mut all = [B1[0]; 22];
        let mut i = 0;
        while i < 4 {
            all[i] = B1[i];
            all[4 + i] = B2[i];
            all[8 + i] = B3[i];
            all[12 + i] = B4[i];
            all[16 + i] = B5[i];
            i += 1;
        }
        all[P_TYPE_LO] = ParamDef {
            id: 50,
            title: "1 Type",
            short_title: "1 Type",
            units: "",
            default_normalized: 0.0,
            step_count: 1,
            can_automate: true,
            to_plain: None,
            from_plain: None,
            format: Some(type_lo_fmt),
        };
        all[P_TYPE_HI] = ParamDef {
            id: 51,
            title: "5 Type",
            short_title: "5 Type",
            units: "",
            default_normalized: 0.0,
            step_count: 1,
            can_automate: true,
            to_plain: None,
            from_plain: None,
            format: Some(type_hi_fmt),
        };
        all
    };

    const METERS: usize = M_SPECTRUM + SPEC_BINS;
    const EDITOR: Option<EditorFactory> = Some(face::make_editor);

    fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            bands: [Band::new(), Band::new(), Band::new(), Band::new(), Band::new()],
            snap: true,
            coef_mid: 0.0,
            env_l: 0.0,
            env_r: 0.0,
            env_decay: 1.0,
            ms_l: 0.0,
            ms_r: 0.0,
            ms_coef: 0.0,
            fft: None,
            ring: Vec::new(),
            widx: 0,
            since_hop: 0,
            hann: Vec::new(),
            re: Vec::new(),
            im: Vec::new(),
            bin_map: Vec::new(),
        }
    }

    fn setup(&mut self, sample_rate: f64, _max_block_size: usize) {
        let sr = sample_rate as f32;
        self.sample_rate = sr;
        self.coef_mid = 1.0 - (-1.0 / (sr * 0.010)).exp();
        self.env_decay = (0.1f32.ln() / (1.5 * sr)).exp();
        self.ms_coef = 1.0 - (-1.0 / (sr * 0.3)).exp();

        self.fft = Some(fft::Fft::new(FFT_N));
        self.ring = vec![0.0; FFT_N];
        self.re = vec![0.0; FFT_N];
        self.im = vec![0.0; FFT_N];
        self.hann = (0..FFT_N)
            .map(|i| {
                0.5 * (1.0
                    - (std::f32::consts::TAU * i as f32 / (FFT_N - 1) as f32).cos())
            })
            .collect();
        // Log-spaced display bins 20..20k; each covers a run of FFT bins.
        self.bin_map = (0..SPEC_BINS)
            .map(|i| {
                let e0 = 20.0 * 1000f32.powf(i as f32 / SPEC_BINS as f32);
                let e1 = 20.0 * 1000f32.powf((i + 1) as f32 / SPEC_BINS as f32);
                let max_k = (FFT_N / 2 - 1) as u32;
                let lo = ((e0 * FFT_N as f32 / sr) as u32).clamp(1, max_k);
                let hi = ((e1 * FFT_N as f32 / sr) as u32).clamp(lo, max_k);
                (lo, hi)
            })
            .collect();
        self.reset();
    }

    fn reset(&mut self) {
        for band in &mut self.bands {
            band.state_l.reset();
            band.state_r.reset();
            band.mix = 0.0;
        }
        self.snap = true;
        self.env_l = 0.0;
        self.env_r = 0.0;
        self.ms_l = 0.0;
        self.ms_r = 0.0;
        self.ring.fill(0.0);
        self.widx = 0;
        self.since_hop = 0;
    }

    fn process(&mut self, buffers: &mut [&mut [f32]], params: &ParamValues, meters: &MeterStore) {
        let sr = self.sample_rate;

        // Per-block: glide the band parameters, rebuild coefficients.
        let mut mix_targets = [0.0f32; BANDS];
        for b in 0..BANDS {
            let mode = self.band_mode(b, params);
            let (ft, gt, qt) = (
                params.plain(p_freq(b)) as f32,
                params.plain(p_gain(b)) as f32,
                params.plain(p_q(b)) as f32,
            );
            let band = &mut self.bands[b];
            if self.snap {
                band.sm_f = ft;
                band.sm_g = gt;
                band.sm_q = qt;
            } else {
                band.sm_f += (ft - band.sm_f) * 0.35;
                band.sm_g += (gt - band.sm_g) * 0.35;
                band.sm_q += (qt - band.sm_q) * 0.35;
            }
            band.coeffs = biquad::coeffs(mode, band.sm_f, band.sm_g, band.sm_q, sr);
            mix_targets[b] = if params.normalized(p_enabled(b)) >= 0.5 {
                1.0
            } else {
                0.0
            };
            if self.snap {
                band.mix = mix_targets[b];
            }
        }
        self.snap = false;

        let (first, rest) = buffers.split_at_mut(1);
        let ch_l = &mut *first[0];
        let mut ch_r = rest.first_mut();
        let num_samples = ch_l.len();

        let mut block_peak_l = 0.0f32;
        let mut block_peak_r = 0.0f32;

        for i in 0..num_samples {
            let in_l = ch_l[i];
            let in_r = ch_r.as_ref().map(|c| c[i]).unwrap_or(in_l);
            let (mut l, mut r) = (in_l, in_r);

            for (b, band) in self.bands.iter_mut().enumerate() {
                band.mix += (mix_targets[b] - band.mix) * self.coef_mid;
                // Filters keep running while faded out so re-enabling is
                // state-warm and clickless.
                let fl = band.state_l.process(&band.coeffs, l);
                let fr = band.state_r.process(&band.coeffs, r);
                if band.mix > 1e-4 {
                    l += (fl - l) * band.mix;
                    r += (fr - r) * band.mix;
                }
            }

            ch_l[i] = l;
            if let Some(ch_r) = ch_r.as_mut() {
                ch_r[i] = r;
            }

            // --- Output metering + spectrum tap ---
            let (al, ar) = (l.abs(), r.abs());
            self.env_l = if al > self.env_l { al } else { self.env_l * self.env_decay };
            self.env_r = if ar > self.env_r { ar } else { self.env_r * self.env_decay };
            block_peak_l = block_peak_l.max(al);
            block_peak_r = block_peak_r.max(ar);
            self.ms_l += (l * l - self.ms_l) * self.ms_coef;
            self.ms_r += (r * r - self.ms_r) * self.ms_coef;

            self.ring[self.widx] = 0.5 * (l + r);
            self.widx = (self.widx + 1) & (FFT_N - 1);
            self.since_hop += 1;
            if self.since_hop >= HOP {
                self.since_hop = 0;
                self.analyze(meters);
            }
        }

        meters.set(M_LEVEL_L, self.env_l as f64);
        meters.set(M_LEVEL_R, self.env_r as f64);
        meters.set(M_PEAK_L, meters.get(M_PEAK_L).max(block_peak_l as f64));
        meters.set(M_PEAK_R, meters.get(M_PEAK_R).max(block_peak_r as f64));
        meters.set(M_RMS_L, self.ms_l.sqrt() as f64);
        meters.set(M_RMS_R, self.ms_r.sqrt() as f64);
        meters.set(M_SAMPLE_RATE, sr as f64);
    }
}

lantern_vst3::export_plugin!(EqDsp);

pub use face::preview_face;
