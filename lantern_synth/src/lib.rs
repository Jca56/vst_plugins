//! Lantern — the polyphonic subtractive synth with FM cross-modulation,
//! resurrected from the nih-plug era onto the Lantern foundation.
//!
//! Built for dubstep bass:
//!   - detuned saws (Osc1 + Osc2)   -> Reese bass
//!   - global LFO -> filter cutoff  -> wobble bass
//!   - FM cross-mod + drive         -> growl / grit
//!
//! 16 voices, quietest-steal allocation, sample-accurate note events.

mod face;
mod synth;

use lantern_vst3::plugin::{
    Dsp, EditorFactory, MeterStore, NoteEvent, NoteKind, ParamDef, ParamValues, PluginInfo,
};
use synth::{naive_wave, EnvStage, Voice, VoiceParams, Waveform};

pub use face::preview_face;

const NUM_VOICES: usize = 16;

// Parameter indices (== ids, forever).
pub const P_O1_WAVE: usize = 0;
pub const P_O2_WAVE: usize = 1;
pub const P_OSC_MIX: usize = 2;
pub const P_O2_DETUNE: usize = 3;
pub const P_O2_OCTAVE: usize = 4;
pub const P_FM: usize = 5;
pub const P_CUTOFF: usize = 6;
pub const P_RES: usize = 7;
pub const P_FENV_AMT: usize = 8;
pub const P_LFO_RATE: usize = 9;
pub const P_LFO_DEPTH: usize = 10;
pub const P_LFO_WAVE: usize = 11;
pub const P_AMP_A: usize = 12;
pub const P_AMP_D: usize = 13;
pub const P_AMP_S: usize = 14;
pub const P_AMP_R: usize = 15;
pub const P_FLT_A: usize = 16;
pub const P_FLT_D: usize = 17;
pub const P_FLT_S: usize = 18;
pub const P_FLT_R: usize = 19;
pub const P_DRIVE: usize = 20;
pub const P_GAIN: usize = 21;

/// Meter slots (pub so the preview harness can stage demo values).
pub const M_LEVEL_L: usize = 0;
pub const M_LEVEL_R: usize = 1;
pub const M_PEAK_L: usize = 2;
pub const M_PEAK_R: usize = 3;
pub const M_RMS_L: usize = 4;
pub const M_RMS_R: usize = 5;
/// Active voice count, for the face.
pub const M_VOICES: usize = 6;

// ============================================================================
// Parameter mappings
// ============================================================================

fn pct_plain(n: f64) -> f64 {
    n * 100.0
}
fn pct_norm(p: f64) -> f64 {
    p / 100.0
}
fn pct_fmt(n: f64) -> String {
    format!("{:.0}", pct_plain(n))
}

fn wave_fmt(n: f64) -> String {
    ["Sine", "Tri", "Saw", "Sqr"][((n * 3.0).round() as usize).min(3)].to_string()
}

fn detune_plain(n: f64) -> f64 {
    n * 200.0 - 100.0
}
fn detune_norm(p: f64) -> f64 {
    (p + 100.0) / 200.0
}
fn detune_fmt(n: f64) -> String {
    format!("{:+.0}", detune_plain(n))
}

fn oct_plain(n: f64) -> f64 {
    (n * 4.0).round() - 2.0
}
fn oct_norm(p: f64) -> f64 {
    (p + 2.0) / 4.0
}
fn oct_fmt(n: f64) -> String {
    format!("{:+.0}", oct_plain(n))
}

fn cutoff_plain(n: f64) -> f64 {
    20.0 * 1000f64.powf(n)
}
fn cutoff_norm(p: f64) -> f64 {
    (p.max(20.0) / 20.0).log10() / 3.0
}
fn cutoff_fmt(n: f64) -> String {
    let f = cutoff_plain(n);
    if f >= 1000.0 {
        format!("{:.1}k", f / 1000.0)
    } else {
        format!("{f:.0}")
    }
}

fn moct_plain(n: f64) -> f64 {
    n * 8.0
}
fn moct_norm(p: f64) -> f64 {
    p / 8.0
}
fn moct_fmt(n: f64) -> String {
    format!("{:.1}", moct_plain(n))
}

fn rate_plain(n: f64) -> f64 {
    0.02 * 2000f64.powf(n) // 0.02 .. 40 Hz, log
}
fn rate_norm(p: f64) -> f64 {
    (p.max(0.02) / 0.02).ln() / 2000f64.ln()
}
fn rate_fmt(n: f64) -> String {
    let p = rate_plain(n);
    if p >= 10.0 {
        format!("{p:.1}")
    } else {
        format!("{p:.2}")
    }
}

fn time_plain(n: f64) -> f64 {
    0.001 * 5000f64.powf(n) // 1 ms .. 5 s, log
}
fn time_norm(p: f64) -> f64 {
    (p.max(0.001) / 0.001).ln() / 5000f64.ln()
}
fn time_fmt(n: f64) -> String {
    let s = time_plain(n);
    if s < 1.0 {
        format!("{:.0} ms", s * 1000.0)
    } else {
        format!("{s:.2} s")
    }
}

fn gain_plain(n: f64) -> f64 {
    n * 60.0 - 60.0
}
fn gain_norm(p: f64) -> f64 {
    (p + 60.0) / 60.0
}
fn gain_fmt(n: f64) -> String {
    format!("{:.1}", gain_plain(n))
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
// The DSP
// ============================================================================

pub struct LanternSynthDsp {
    sample_rate: f32,
    voices: [Voice; NUM_VOICES],
    lfo_phase: f32,
    // One-pole smoothed continuous controls (zipper-noise control).
    sm: [f32; 10],
    snap: bool,
    coef: f32,
    // Output metering (family standard).
    env_l: f32,
    env_decay: f32,
    ms_l: f32,
    ms_coef: f32,
}

/// Indices into the smoother array.
const S_MIX: usize = 0;
const S_DETUNE: usize = 1;
const S_FM: usize = 2;
const S_CUTOFF: usize = 3;
const S_RES: usize = 4;
const S_FENV: usize = 5;
const S_LFO_RATE: usize = 6;
const S_LFO_DEPTH: usize = 7;
const S_DRIVE: usize = 8;
const S_GAIN: usize = 9;

impl LanternSynthDsp {
    fn note_on(&mut self, note: u8, velocity: f32) {
        // Take a free voice, else steal the quietest one.
        let idx = self
            .voices
            .iter()
            .position(|v| !v.active)
            .unwrap_or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.amp_env.level.total_cmp(&b.amp_env.level))
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            });
        self.voices[idx].note_on(note, velocity);
    }

    fn note_off(&mut self, note: u8) {
        for v in self.voices.iter_mut() {
            if v.active && v.note == note && v.amp_env.stage != EnvStage::Release {
                v.note_off();
            }
        }
    }
}

impl Dsp for LanternSynthDsp {
    const INFO: PluginInfo = PluginInfo {
        name: "Lantern",
        vendor: "Alva",
        version: "0.2.0",
        url: "https://github.com/",
        email: "noreply@example.com",
        class_id: *b"LanternSynth2Alv",
        subcategories: "Instrument|Synth",
    };

    #[rustfmt::skip]
    const PARAMS: &'static [ParamDef] = &[
        p!(0,  "Osc1 Wave",  "",    2.0 / 3.0, 3, None, None, Some(wave_fmt)),
        p!(1,  "Osc2 Wave",  "",    2.0 / 3.0, 3, None, None, Some(wave_fmt)),
        p!(2,  "Osc Mix",    "%",   0.5,    0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(3,  "Osc2 Detune","ct",  0.56,   0, Some(detune_plain), Some(detune_norm), Some(detune_fmt)),
        p!(4,  "Osc2 Octave","oct", 0.5,    4, Some(oct_plain), Some(oct_norm), Some(oct_fmt)),
        p!(5,  "FM Amount",  "%",   0.0,    0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(6,  "Cutoff",     "Hz",  0.5340, 0, Some(cutoff_plain), Some(cutoff_norm), Some(cutoff_fmt)),
        p!(7,  "Resonance",  "%",   0.3,    0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(8,  "Filter Env", "oct", 0.375,  0, Some(moct_plain), Some(moct_norm), Some(moct_fmt)),
        p!(9,  "LFO Rate",   "Hz",  0.6972, 0, Some(rate_plain), Some(rate_norm), Some(rate_fmt)),
        p!(10, "LFO Depth",  "oct", 0.0,    0, Some(moct_plain), Some(moct_norm), Some(moct_fmt)),
        p!(11, "LFO Wave",   "",    0.0,    3, None, None, Some(wave_fmt)),
        p!(12, "Amp Attack", "",    0.1889, 0, Some(time_plain), Some(time_norm), Some(time_fmt)),
        p!(13, "Amp Decay",  "",    0.6696, 0, Some(time_plain), Some(time_norm), Some(time_fmt)),
        p!(14, "Amp Sustain","%",   0.9,    0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(15, "Amp Release","",    0.5883, 0, Some(time_plain), Some(time_norm), Some(time_fmt)),
        p!(16, "Flt Attack", "",    0.0814, 0, Some(time_plain), Some(time_norm), Some(time_fmt)),
        p!(17, "Flt Decay",  "",    0.6482, 0, Some(time_plain), Some(time_norm), Some(time_fmt)),
        p!(18, "Flt Sustain","%",   0.2,    0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(19, "Flt Release","",    0.6220, 0, Some(time_plain), Some(time_norm), Some(time_fmt)),
        p!(20, "Drive",      "%",   0.1,    0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(21, "Gain",       "dB",  0.9,    0, Some(gain_plain), Some(gain_norm), Some(gain_fmt)),
    ];

    const METERS: usize = 7;
    const EDITOR: Option<EditorFactory> = Some(face::make_editor);
    const IS_INSTRUMENT: bool = true;

    fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            voices: std::array::from_fn(|_| Voice::new()),
            lfo_phase: 0.0,
            sm: [0.0; 10],
            snap: true,
            coef: 0.0,
            env_l: 0.0,
            env_decay: 1.0,
            ms_l: 0.0,
            ms_coef: 0.0,
        }
    }

    fn setup(&mut self, sample_rate: f64, _max_block_size: usize) {
        let sr = sample_rate as f32;
        self.sample_rate = sr;
        self.coef = 1.0 - (-1.0 / (sr * 0.010)).exp();
        self.env_decay = (0.1f32.ln() / (1.5 * sr)).exp();
        self.ms_coef = 1.0 - (-1.0 / (sr * 0.3)).exp();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            *v = Voice::new();
        }
        self.lfo_phase = 0.0;
        self.snap = true;
        self.env_l = 0.0;
        self.ms_l = 0.0;
    }

    fn process_with_events(
        &mut self,
        buffers: &mut [&mut [f32]],
        events: &[NoteEvent],
        params: &ParamValues,
        meters: &MeterStore,
    ) {
        let sr = self.sample_rate;

        // Per-block targets for the smoothed continuous controls.
        let targets = [
            params.plain(P_OSC_MIX) as f32 / 100.0,
            params.plain(P_O2_DETUNE) as f32,
            params.plain(P_FM) as f32 / 100.0,
            params.plain(P_CUTOFF) as f32,
            params.plain(P_RES) as f32 / 100.0,
            params.plain(P_FENV_AMT) as f32,
            params.plain(P_LFO_RATE) as f32,
            params.plain(P_LFO_DEPTH) as f32,
            params.plain(P_DRIVE) as f32 / 100.0,
            10f32.powf(params.plain(P_GAIN) as f32 / 20.0),
        ];
        if self.snap {
            self.sm = targets;
            self.snap = false;
        }

        // Per-block discrete values.
        let osc1_wave = Waveform::from_index((params.normalized(P_O1_WAVE) * 3.0).round() as usize);
        let osc2_wave = Waveform::from_index((params.normalized(P_O2_WAVE) * 3.0).round() as usize);
        let lfo_wave = Waveform::from_index((params.normalized(P_LFO_WAVE) * 3.0).round() as usize);
        let osc2_octave = params.plain(P_O2_OCTAVE) as i32;
        let (amp_a, amp_d, amp_s, amp_r) = (
            params.plain(P_AMP_A) as f32,
            params.plain(P_AMP_D) as f32,
            params.plain(P_AMP_S) as f32 / 100.0,
            params.plain(P_AMP_R) as f32,
        );
        let (flt_a, flt_d, flt_s, flt_r) = (
            params.plain(P_FLT_A) as f32,
            params.plain(P_FLT_D) as f32,
            params.plain(P_FLT_S) as f32 / 100.0,
            params.plain(P_FLT_R) as f32,
        );

        let (first, rest) = buffers.split_at_mut(1);
        let ch_l = &mut *first[0];
        let mut ch_r = rest.first_mut();
        let num_samples = ch_l.len();

        let mut next_event = 0usize;
        let mut block_peak = 0.0f32;

        for i in 0..num_samples {
            // Sample-accurate note events.
            while next_event < events.len() && events[next_event].sample_offset as usize <= i {
                match events[next_event].kind {
                    NoteKind::On { pitch, velocity } => self.note_on(pitch, velocity),
                    NoteKind::Off { pitch } => self.note_off(pitch),
                }
                next_event += 1;
            }

            for (s, t) in self.sm.iter_mut().zip(targets.iter()) {
                *s += (t - *s) * self.coef;
            }

            let vp = VoiceParams {
                osc1_wave,
                osc2_wave,
                osc_mix: self.sm[S_MIX],
                osc2_detune: self.sm[S_DETUNE],
                osc2_octave,
                fm_amount: self.sm[S_FM],
                cutoff: self.sm[S_CUTOFF],
                resonance: self.sm[S_RES],
                filt_env_oct: self.sm[S_FENV],
                lfo_oct: self.sm[S_LFO_DEPTH],
                amp_a,
                amp_d,
                amp_s,
                amp_r,
                flt_a,
                flt_d,
                flt_s,
                flt_r,
                sr,
            };

            // Global LFO (shared across voices) — the wobble.
            let lfo_value = naive_wave(lfo_wave, self.lfo_phase);
            self.lfo_phase = (self.lfo_phase + self.sm[S_LFO_RATE] / sr).fract();

            let mut sum = 0.0;
            for v in self.voices.iter_mut() {
                sum += v.render(&vp, lfo_value);
            }

            // Soft-clip (drive) then output gain; tanh keeps it bounded.
            let out = (sum * (1.0 + self.sm[S_DRIVE] * 24.0)).tanh() * self.sm[S_GAIN];

            ch_l[i] = out;
            if let Some(ch_r) = ch_r.as_mut() {
                ch_r[i] = out;
            }

            let a = out.abs();
            self.env_l = if a > self.env_l { a } else { self.env_l * self.env_decay };
            block_peak = block_peak.max(a);
            self.ms_l += (out * out - self.ms_l) * self.ms_coef;
        }

        let rms = self.ms_l.sqrt() as f64;
        meters.set(M_LEVEL_L, self.env_l as f64);
        meters.set(M_LEVEL_R, self.env_l as f64);
        meters.set(M_PEAK_L, meters.get(M_PEAK_L).max(block_peak as f64));
        meters.set(M_PEAK_R, meters.get(M_PEAK_R).max(block_peak as f64));
        meters.set(M_RMS_L, rms);
        meters.set(M_RMS_R, rms);
        meters.set(
            M_VOICES,
            self.voices.iter().filter(|v| v.active).count() as f64,
        );
    }
}

lantern_vst3::export_plugin!(LanternSynthDsp);
