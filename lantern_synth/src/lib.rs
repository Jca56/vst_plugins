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
use synth::{
    naive_wave, EnvStage, FilterMode, OscSettings, Voice, VoiceParams, Waveform, MAX_UNISON,
};

pub use face::{background, preview_face};

const NUM_VOICES: usize = 16;

// Parameter indices (== ids; renumbered once while Blacklight was young and
// no projects existed — from here on they are forever).
// Per-oscillator block: o in {0, 1}, six params each.
pub const OSC_ENABLE: usize = 0;
pub const OSC_WAVE: usize = 1;
pub const OSC_VOLUME: usize = 2;
pub const OSC_PITCH: usize = 3;
pub const OSC_VOICES: usize = 4;
pub const OSC_DETUNE: usize = 5;
pub fn p_osc(o: usize, which: usize) -> usize {
    o * 6 + which
}
pub const P_FM: usize = 12;
pub const P_CUTOFF: usize = 13;
pub const P_RES: usize = 14;
pub const P_FENV_AMT: usize = 15;
pub const P_LFO_RATE: usize = 16;
pub const P_LFO_DEPTH: usize = 17;
pub const P_LFO_WAVE: usize = 18;
pub const P_AMP_A: usize = 19;
pub const P_AMP_D: usize = 20;
pub const P_AMP_S: usize = 21;
pub const P_AMP_R: usize = 22;
pub const P_FLT_A: usize = 23;
pub const P_FLT_D: usize = 24;
pub const P_FLT_S: usize = 25;
pub const P_FLT_R: usize = 26;
pub const P_DRIVE: usize = 27;
pub const P_GAIN: usize = 28;
pub const P_FLT_TYPE: usize = 29;
pub const P_FLT_ON: usize = 30;

/// Meter slots (pub so the preview harness can stage demo values).
pub const M_LEVEL_L: usize = 0;
pub const M_LEVEL_R: usize = 1;
pub const M_PEAK_L: usize = 2;
pub const M_PEAK_R: usize = 3;
pub const M_RMS_L: usize = 4;
pub const M_RMS_R: usize = 5;
/// Active voice count, for the face.
pub const M_VOICES: usize = 6;
/// Bitmask of sounding keys on the face keyboard: bit i = MIDI note 24+i
/// (C0..=C3 in Live naming); out-of-range notes fold by octaves into view.
pub const M_KEYS: usize = 7;
/// UI keyboard gate, written by the editor: note+1 while a key is held,
/// 0 when released. The DSP edge-detects it into note on/off.
pub const M_UI_NOTE: usize = 8;

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

fn osc_wave_fmt(n: f64) -> String {
    ["Sine", "Triangle", "Saw", "Square", "Pulse 25%", "Pulse 12%", "Shark", "Noise"]
        [((n * 7.0).round() as usize).min(7)]
    .to_string()
}

fn lfo_wave_fmt(n: f64) -> String {
    ["Sine", "Tri", "Saw", "Sqr"][((n * 3.0).round() as usize).min(3)].to_string()
}

fn flt_type_fmt(n: f64) -> String {
    ["Lowpass", "Bandpass", "Highpass", "Notch"][((n * 3.0).round() as usize).min(3)].to_string()
}

fn detune_plain(n: f64) -> f64 {
    n * 100.0 // unison spread, 0..100 cents
}
fn detune_norm(p: f64) -> f64 {
    p / 100.0
}
fn detune_fmt(n: f64) -> String {
    format!("{:.0}", detune_plain(n))
}

fn pitch_plain(n: f64) -> f64 {
    (n * 8.0).round() - 4.0 // whole octaves, +/- 4 — keys stay keys
}
fn pitch_norm(p: f64) -> f64 {
    (p + 4.0) / 8.0
}
fn pitch_fmt(n: f64) -> String {
    let p = pitch_plain(n);
    if p == 0.0 {
        "0".to_string()
    } else {
        format!("{p:+.0}")
    }
}

fn voices_plain(n: f64) -> f64 {
    (n * 6.0).round() + 1.0 // 1..7 unison voices
}
fn voices_norm(p: f64) -> f64 {
    (p - 1.0) / 6.0
}
fn voices_fmt(n: f64) -> String {
    format!("{:.0}", voices_plain(n))
}

fn on_fmt(n: f64) -> String {
    if n >= 0.5 { "On" } else { "Off" }.to_string()
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
    sm: [f32; 12],
    snap: bool,
    coef: f32,
    // Output metering (family standard).
    env_l: f32,
    env_decay: f32,
    ms_l: f32,
    ms_coef: f32,
    /// Last seen UI-keyboard gate (note+1, 0 = none), for edge detection.
    ui_note_prev: i32,
}

/// Indices into the smoother array.
const S_VOL0: usize = 0;
const S_VOL1: usize = 1;
const S_DET0: usize = 2;
const S_DET1: usize = 3;
const S_FM: usize = 4;
const S_CUTOFF: usize = 5;
const S_RES: usize = 6;
const S_FENV: usize = 7;
const S_LFO_RATE: usize = 8;
const S_LFO_DEPTH: usize = 9;
const S_DRIVE: usize = 10;
const S_GAIN: usize = 11;

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
        name: "Lantern Blacklight",
        vendor: "Alva",
        version: "0.3.0",
        url: "https://github.com/",
        email: "noreply@example.com",
        class_id: *b"LanternBlacklite",
        subcategories: "Instrument|Synth",
    };

    #[rustfmt::skip]
    const PARAMS: &'static [ParamDef] = &[
        p!(0,  "O1 On",     "",   1.0,       1, None, None, Some(on_fmt)),
        p!(1,  "O1 Wave",   "",   2.0 / 7.0, 7, None, None, Some(osc_wave_fmt)),
        p!(2,  "O1 Volume", "%",  0.7,       0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(3,  "O1 Pitch",  "oct", 0.5,      8, Some(pitch_plain), Some(pitch_norm), Some(pitch_fmt)),
        p!(4,  "O1 Voices", "",   0.0,       6, Some(voices_plain), Some(voices_norm), Some(voices_fmt)),
        p!(5,  "O1 Detune", "ct", 0.10,      0, Some(detune_plain), Some(detune_norm), Some(detune_fmt)),
        p!(6,  "O2 On",     "",   1.0,       1, None, None, Some(on_fmt)),
        p!(7,  "O2 Wave",   "",   2.0 / 7.0, 7, None, None, Some(osc_wave_fmt)),
        p!(8,  "O2 Volume", "%",  0.7,       0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(9,  "O2 Pitch",  "oct", 0.5,      8, Some(pitch_plain), Some(pitch_norm), Some(pitch_fmt)),
        p!(10, "O2 Voices", "",   1.0 / 6.0, 6, Some(voices_plain), Some(voices_norm), Some(voices_fmt)),
        p!(11, "O2 Detune", "ct", 0.12,      0, Some(detune_plain), Some(detune_norm), Some(detune_fmt)),
        p!(12, "FM Amount", "%",  0.0,       0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(13, "Cutoff",    "Hz", 0.5340,    0, Some(cutoff_plain), Some(cutoff_norm), Some(cutoff_fmt)),
        p!(14, "Resonance", "%",  0.3,       0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(15, "Filter Env","oct",0.375,     0, Some(moct_plain), Some(moct_norm), Some(moct_fmt)),
        p!(16, "LFO Rate",  "Hz", 0.6972,    0, Some(rate_plain), Some(rate_norm), Some(rate_fmt)),
        p!(17, "LFO Depth", "oct",0.0,       0, Some(moct_plain), Some(moct_norm), Some(moct_fmt)),
        p!(18, "LFO Wave",  "",   0.0,       3, None, None, Some(lfo_wave_fmt)),
        p!(19, "Amp Attack","",   0.1889,    0, Some(time_plain), Some(time_norm), Some(time_fmt)),
        p!(20, "Amp Decay", "",   0.6696,    0, Some(time_plain), Some(time_norm), Some(time_fmt)),
        p!(21, "Amp Sustain","%", 0.9,       0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(22, "Amp Release","",  0.5883,    0, Some(time_plain), Some(time_norm), Some(time_fmt)),
        p!(23, "Flt Attack","",   0.0814,    0, Some(time_plain), Some(time_norm), Some(time_fmt)),
        p!(24, "Flt Decay", "",   0.6482,    0, Some(time_plain), Some(time_norm), Some(time_fmt)),
        p!(25, "Flt Sustain","%", 0.2,       0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(26, "Flt Release","",  0.6220,    0, Some(time_plain), Some(time_norm), Some(time_fmt)),
        p!(27, "Drive",     "%",  0.1,       0, Some(pct_plain), Some(pct_norm), Some(pct_fmt)),
        p!(28, "Gain",      "dB", 0.9,       0, Some(gain_plain), Some(gain_norm), Some(gain_fmt)),
        p!(29, "Flt Type",  "",   0.0,       3, None, None, Some(flt_type_fmt)),
        p!(30, "Flt On",    "",   1.0,       1, None, None, Some(on_fmt)),
    ];

    const METERS: usize = 9;
    const EDITOR: Option<EditorFactory> = Some(face::make_editor);
    const IS_INSTRUMENT: bool = true;

    fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            voices: std::array::from_fn(|_| Voice::new()),
            lfo_phase: 0.0,
            sm: [0.0; 12],
            snap: true,
            coef: 0.0,
            env_l: 0.0,
            env_decay: 1.0,
            ms_l: 0.0,
            ms_coef: 0.0,
            ui_note_prev: 0,
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
        self.ui_note_prev = 0;
    }

    fn process_with_events(
        &mut self,
        buffers: &mut [&mut [f32]],
        events: &[NoteEvent],
        params: &ParamValues,
        meters: &MeterStore,
    ) {
        let sr = self.sample_rate;

        // The editor's mini keyboard: edge-detect the gate slot into notes.
        let ui_gate = meters.get(M_UI_NOTE) as i32;
        if ui_gate != self.ui_note_prev {
            if self.ui_note_prev > 0 {
                self.note_off((self.ui_note_prev - 1).clamp(0, 127) as u8);
            }
            if ui_gate > 0 {
                self.note_on((ui_gate - 1).clamp(0, 127) as u8, 0.9);
            }
            self.ui_note_prev = ui_gate;
        }

        // Per-block targets for the smoothed continuous controls.
        let targets = [
            params.plain(p_osc(0, OSC_VOLUME)) as f32 / 100.0,
            params.plain(p_osc(1, OSC_VOLUME)) as f32 / 100.0,
            params.plain(p_osc(0, OSC_DETUNE)) as f32,
            params.plain(p_osc(1, OSC_DETUNE)) as f32,
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

        // Per-block oscillator settings: pitch + unison detune spread baked
        // into per-copy frequency ratios (powf lives here, not per sample).
        let mut osc_settings = [OscSettings {
            enabled: false,
            wave: Waveform::Saw,
            volume: 0.0,
            voices: 1,
            ratios: [1.0; MAX_UNISON],
        }; 2];
        for (o, set) in osc_settings.iter_mut().enumerate() {
            set.enabled = params.normalized(p_osc(o, OSC_ENABLE)) >= 0.5;
            set.wave = Waveform::from_index(
                (params.normalized(p_osc(o, OSC_WAVE)) * 7.0).round() as usize,
            );
            set.voices = (params.plain(p_osc(o, OSC_VOICES)) as usize).clamp(1, MAX_UNISON);
            let pitch_oct = params.plain(p_osc(o, OSC_PITCH)) as f32;
            let detune_ct = self.sm[S_DET0 + o];
            for k in 0..MAX_UNISON {
                let spread = if set.voices <= 1 || k >= set.voices {
                    0.0
                } else {
                    k as f32 / (set.voices - 1) as f32 * 2.0 - 1.0
                };
                set.ratios[k] = 2.0f32.powf(pitch_oct + detune_ct * spread / 1200.0);
            }
        }

        // Per-block discrete values.
        let lfo_wave = Waveform::from_index((params.normalized(P_LFO_WAVE) * 3.0).round() as usize);
        let flt_on = params.normalized(P_FLT_ON) >= 0.5;
        let flt_mode =
            FilterMode::from_index((params.normalized(P_FLT_TYPE) * 3.0).round() as usize);
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

            let mut osc = osc_settings;
            osc[0].volume = self.sm[S_VOL0];
            osc[1].volume = self.sm[S_VOL1];
            let vp = VoiceParams {
                osc,
                fm_amount: self.sm[S_FM],
                flt_on,
                flt_mode,
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
        let mut key_mask = 0u64;
        for v in self.voices.iter().filter(|v| v.active) {
            let mut n = v.note as i32;
            while n < 24 {
                n += 12;
            }
            while n > 60 {
                n -= 12;
            }
            key_mask |= 1 << (n - 24);
        }
        meters.set(M_KEYS, key_mask as f64);
    }
}

lantern_vst3::export_plugin!(LanternSynthDsp);
