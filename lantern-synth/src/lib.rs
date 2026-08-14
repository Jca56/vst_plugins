//! Lantern — a polyphonic subtractive synth with FM cross-modulation.
//!
//! Architecture (signal flow per voice):
//!   Osc1 + Osc2  ->  Osc2 phase-modulates Osc1 (FM)  ->  mix  ->  resonant LP filter
//!   filter cutoff is modulated by a per-voice filter envelope AND a global LFO
//!   then  amp envelope * velocity.  Voices are summed, soft-clipped (drive), gained.
//!
//! Built for dubstep bass:
//!   - detuned saws (Osc1 + Osc2)               -> Reese bass
//!   - global LFO -> filter cutoff              -> wobble bass
//!   - FM cross-mod + drive                     -> growl / grit
//!
//! Exports both VST3 (for Ableton-under-Wine) and CLAP.

use nih_plug::prelude::*;
use std::f32::consts::{PI, TAU};
use std::num::NonZeroU32;
use std::sync::Arc;

use nih_plug_vizia::ViziaState;

mod editor;

const NUM_VOICES: usize = 16;

// ============================================================================
// Plugin state
// ============================================================================

pub struct LanternSynth {
    params: Arc<LanternSynthParams>,
    voices: [Voice; NUM_VOICES],
    sample_rate: f32,
    /// Global LFO phase in [0, 1).
    lfo_phase: f32,
}

impl Default for LanternSynth {
    fn default() -> Self {
        Self {
            params: Arc::new(LanternSynthParams::default()),
            voices: std::array::from_fn(|_| Voice::new()),
            sample_rate: 44_100.0,
            lfo_phase: 0.0,
        }
    }
}

// ============================================================================
// Parameters
// ============================================================================

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
enum Waveform {
    #[name = "Sine"]
    Sine,
    #[name = "Triangle"]
    Triangle,
    #[name = "Saw"]
    Saw,
    #[name = "Square"]
    Square,
}

#[derive(Params)]
struct LanternSynthParams {
    /// Editor (window) state, persisted alongside the patch.
    #[persist = "editor-state"]
    editor_state: Arc<ViziaState>,

    // --- Oscillators ---
    #[id = "o1wave"]
    osc1_wave: EnumParam<Waveform>,
    #[id = "o2wave"]
    osc2_wave: EnumParam<Waveform>,
    #[id = "oscmix"]
    osc_mix: FloatParam,
    #[id = "o2det"]
    osc2_detune: FloatParam,
    #[id = "o2oct"]
    osc2_octave: IntParam,
    #[id = "fm"]
    fm_amount: FloatParam,

    // --- Filter ---
    #[id = "cutoff"]
    cutoff: FloatParam,
    #[id = "res"]
    resonance: FloatParam,
    #[id = "fenv"]
    filt_env_amount: FloatParam,

    // --- LFO (the wobble) ---
    #[id = "lforate"]
    lfo_rate: FloatParam,
    #[id = "lfodepth"]
    lfo_depth: FloatParam,
    #[id = "lfowave"]
    lfo_wave: EnumParam<Waveform>,

    // --- Amp envelope ---
    #[id = "aatk"]
    amp_attack: FloatParam,
    #[id = "adec"]
    amp_decay: FloatParam,
    #[id = "asus"]
    amp_sustain: FloatParam,
    #[id = "arel"]
    amp_release: FloatParam,

    // --- Filter envelope ---
    #[id = "fatk"]
    flt_attack: FloatParam,
    #[id = "fdec"]
    flt_decay: FloatParam,
    #[id = "fsus"]
    flt_sustain: FloatParam,
    #[id = "frel"]
    flt_release: FloatParam,

    // --- Output ---
    #[id = "drive"]
    drive: FloatParam,
    #[id = "gain"]
    gain: FloatParam,
}

/// Skewed time range used by every ADSR stage: lots of resolution at short times.
fn time_range() -> FloatRange {
    FloatRange::Skewed {
        min: 0.001,
        max: 5.0,
        factor: FloatRange::skew_factor(-2.0),
    }
}

/// A 0..1 "percentage" parameter with a %-formatter.
fn percent(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_smoother(SmoothingStyle::Linear(20.0))
        .with_unit(" %")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

impl Default for LanternSynthParams {
    fn default() -> Self {
        Self {
            editor_state: editor::default_state(),

            // Oscillators — two saws, slightly detuned = instant Reese.
            osc1_wave: EnumParam::new("Osc 1 Wave", Waveform::Saw),
            osc2_wave: EnumParam::new("Osc 2 Wave", Waveform::Saw),
            osc_mix: percent("Osc Mix", 0.5),
            osc2_detune: FloatParam::new(
                "Osc 2 Detune",
                12.0,
                FloatRange::Linear {
                    min: -100.0,
                    max: 100.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_unit(" ct"),
            osc2_octave: IntParam::new("Osc 2 Octave", 0, IntRange::Linear { min: -2, max: 2 }),
            fm_amount: percent("FM Amount", 0.0),

            // Filter — resonant low-pass, the source of all movement.
            cutoff: FloatParam::new(
                "Cutoff",
                800.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(20.0))
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(1))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            resonance: percent("Resonance", 0.3),
            filt_env_amount: FloatParam::new(
                "Filter Env",
                3.0,
                FloatRange::Linear { min: 0.0, max: 8.0 },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_unit(" oct"),

            // LFO — route depth up for the classic wub-wub.
            lfo_rate: FloatParam::new(
                "LFO Rate",
                4.0,
                FloatRange::Skewed {
                    min: 0.01,
                    max: 40.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_unit(" Hz"),
            lfo_depth: FloatParam::new("LFO Depth", 0.0, FloatRange::Linear { min: 0.0, max: 8.0 })
                .with_smoother(SmoothingStyle::Linear(20.0))
                .with_unit(" oct"),
            lfo_wave: EnumParam::new("LFO Wave", Waveform::Sine),

            // Amp envelope — sustained by default.
            amp_attack: FloatParam::new("Amp Attack", 0.005, time_range()).with_unit(" s"),
            amp_decay: FloatParam::new("Amp Decay", 0.3, time_range()).with_unit(" s"),
            amp_sustain: percent("Amp Sustain", 0.9),
            amp_release: FloatParam::new("Amp Release", 0.15, time_range()).with_unit(" s"),

            // Filter envelope — snappy pluck (sustain low) for a filter sweep on each note.
            flt_attack: FloatParam::new("Filter Attack", 0.002, time_range()).with_unit(" s"),
            flt_decay: FloatParam::new("Filter Decay", 0.25, time_range()).with_unit(" s"),
            flt_sustain: percent("Filter Sustain", 0.2),
            flt_release: FloatParam::new("Filter Release", 0.2, time_range()).with_unit(" s"),

            // Output.
            drive: percent("Drive", 0.1),
            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(-6.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-60.0),
                    max: util::db_to_gain(0.0),
                    factor: FloatRange::gain_skew_factor(-60.0, 0.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
        }
    }
}

// ============================================================================
// Voice + DSP
// ============================================================================

/// Per-sample scalar snapshot of all the params a voice needs. Built once per
/// sample so voices don't have to touch the smoothers (which must advance once
/// per sample, not once per voice).
struct VoiceParams {
    osc1_wave: Waveform,
    osc2_wave: Waveform,
    osc_mix: f32,
    osc2_detune: f32,
    osc2_octave: i32,
    fm_amount: f32,
    cutoff: f32,
    resonance: f32,
    filt_env_oct: f32,
    lfo_oct: f32,
    amp_a: f32,
    amp_d: f32,
    amp_s: f32,
    amp_r: f32,
    flt_a: f32,
    flt_d: f32,
    flt_s: f32,
    flt_r: f32,
    sr: f32,
}

struct Voice {
    active: bool,
    note: u8,
    velocity: f32,
    phase1: f32,
    phase2: f32,
    amp_env: Adsr,
    flt_env: Adsr,
    filter: Svf,
}

impl Voice {
    fn new() -> Self {
        Self {
            active: false,
            note: 0,
            velocity: 0.0,
            phase1: 0.0,
            phase2: 0.0,
            amp_env: Adsr::new(),
            flt_env: Adsr::new(),
            filter: Svf::new(),
        }
    }

    fn note_on(&mut self, note: u8, velocity: f32) {
        self.active = true;
        self.note = note;
        self.velocity = velocity;
        self.phase1 = 0.0;
        self.phase2 = 0.0;
        self.filter.reset();
        self.amp_env.trigger();
        self.flt_env.trigger();
    }

    fn note_off(&mut self) {
        self.amp_env.release();
        self.flt_env.release();
    }

    fn render(&mut self, p: &VoiceParams, lfo_value: f32) -> f32 {
        if !self.active {
            return 0.0;
        }

        let amp = self.amp_env.process(p.amp_a, p.amp_d, p.amp_s, p.amp_r, p.sr);
        let fenv = self.flt_env.process(p.flt_a, p.flt_d, p.flt_s, p.flt_r, p.sr);
        // The amp envelope owns the voice's lifetime.
        if self.amp_env.stage == EnvStage::Idle {
            self.active = false;
            return 0.0;
        }

        let base = midi_to_freq(self.note);
        let f1 = base;
        let f2 = base * 2.0f32.powi(p.osc2_octave) * 2.0f32.powf(p.osc2_detune / 1200.0);
        let dt1 = (f1 / p.sr).min(0.49);
        let dt2 = (f2 / p.sr).min(0.49);

        // Osc 2 phase-modulates Osc 1 (this is the "FM").
        let s2 = osc(p.osc2_wave, self.phase2, dt2);
        let pm = (self.phase1 + s2 * p.fm_amount).rem_euclid(1.0);
        let s1 = osc(p.osc1_wave, pm, dt1);
        let mixed = s1 * (1.0 - p.osc_mix) + s2 * p.osc_mix;

        self.phase1 = (self.phase1 + dt1).fract();
        self.phase2 = (self.phase2 + dt2).fract();

        // Modulate cutoff in octaves (musical) from the filter env + global LFO.
        let mod_oct = p.filt_env_oct * fenv + p.lfo_oct * lfo_value;
        let cutoff = (p.cutoff * 2.0f32.powf(mod_oct)).clamp(20.0, p.sr * 0.45);
        let filtered = self.filter.process_lp(mixed, cutoff, p.resonance, p.sr);

        filtered * amp * self.velocity
    }
}

// --- ADSR envelope (linear segments) ---

#[derive(Clone, Copy, PartialEq)]
enum EnvStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Copy)]
struct Adsr {
    stage: EnvStage,
    level: f32,
}

impl Adsr {
    fn new() -> Self {
        Self {
            stage: EnvStage::Idle,
            level: 0.0,
        }
    }

    fn trigger(&mut self) {
        self.stage = EnvStage::Attack;
    }

    fn release(&mut self) {
        if self.stage != EnvStage::Idle {
            self.stage = EnvStage::Release;
        }
    }

    fn process(&mut self, a: f32, d: f32, s: f32, r: f32, sr: f32) -> f32 {
        match self.stage {
            EnvStage::Idle => self.level = 0.0,
            EnvStage::Attack => {
                self.level += 1.0 / (a.max(1e-4) * sr);
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = EnvStage::Decay;
                }
            }
            EnvStage::Decay => {
                self.level -= (1.0 - s) / (d.max(1e-4) * sr);
                if self.level <= s {
                    self.level = s;
                    self.stage = EnvStage::Sustain;
                }
            }
            EnvStage::Sustain => self.level = s,
            EnvStage::Release => {
                self.level -= 1.0 / (r.max(1e-4) * sr);
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = EnvStage::Idle;
                }
            }
        }
        self.level
    }
}

// --- Cytomic / Andrew Simper TPT state-variable filter (low-pass) ---

#[derive(Clone, Copy)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    fn new() -> Self {
        Self { ic1: 0.0, ic2: 0.0 }
    }

    fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    fn process_lp(&mut self, v0: f32, cutoff: f32, res: f32, sr: f32) -> f32 {
        let g = (PI * cutoff / sr).tan();
        // res in [0,1] -> k = 1/Q from 2 (gentle) down to ~0.02 (screaming).
        let k = (2.0 - 1.98 * res).max(0.02);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = v0 - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v2 // low-pass output
    }
}

// --- Oscillator helpers ---

fn midi_to_freq(note: u8) -> f32 {
    440.0 * 2.0f32.powf((note as f32 - 69.0) / 12.0)
}

/// PolyBLEP correction to band-limit the discontinuities in saw/square.
fn poly_blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        return 0.0;
    }
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// Band-limited oscillator (for audio rate). `dt` is the per-sample phase step.
fn osc(wave: Waveform, phase: f32, dt: f32) -> f32 {
    match wave {
        Waveform::Sine => (phase * TAU).sin(),
        Waveform::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
        Waveform::Saw => (2.0 * phase - 1.0) - poly_blep(phase, dt),
        Waveform::Square => {
            let naive = if phase < 0.5 { 1.0 } else { -1.0 };
            naive + poly_blep(phase, dt) - poly_blep((phase + 0.5).fract(), dt)
        }
    }
}

/// Naive (non-band-limited) shape — fine for the sub-audio LFO.
fn naive_wave(wave: Waveform, phase: f32) -> f32 {
    match wave {
        Waveform::Sine => (phase * TAU).sin(),
        Waveform::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
        Waveform::Saw => 2.0 * phase - 1.0,
        Waveform::Square => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
    }
}

// ============================================================================
// Plugin impl
// ============================================================================

impl Plugin for LanternSynth {
    const NAME: &'static str = "Lantern";
    const VENDOR: &'static str = "Alva";
    const URL: &'static str = "https://github.com/";
    const EMAIL: &'static str = "noreply@example.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    // No audio input, stereo output: it's an instrument.
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.params.editor_state.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        true
    }

    fn reset(&mut self) {
        for voice in self.voices.iter_mut() {
            *voice = Voice::new();
        }
        self.lfo_phase = 0.0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let sr = self.sample_rate;
        let mut next_event = context.next_event();

        for (sample_id, channel_samples) in buffer.iter_samples().enumerate() {
            // Apply all note events scheduled for this sample.
            while let Some(event) = next_event {
                if event.timing() > sample_id as u32 {
                    break;
                }
                match event {
                    NoteEvent::NoteOn { note, velocity, .. } => {
                        // Take a free voice, else steal the quietest one.
                        let idx = self
                            .voices
                            .iter()
                            .position(|v| !v.active)
                            .unwrap_or_else(|| {
                                self.voices
                                    .iter()
                                    .enumerate()
                                    .min_by(|(_, a), (_, b)| {
                                        a.amp_env.level.total_cmp(&b.amp_env.level)
                                    })
                                    .map(|(i, _)| i)
                                    .unwrap_or(0)
                            });
                        self.voices[idx].note_on(note, velocity);
                    }
                    NoteEvent::NoteOff { note, .. } => {
                        for v in self.voices.iter_mut() {
                            if v.active && v.note == note && v.amp_env.stage != EnvStage::Release {
                                v.note_off();
                            }
                        }
                    }
                    _ => {}
                }
                next_event = context.next_event();
            }

            // Snapshot params for this sample (smoothers must advance exactly once/sample).
            let gain = self.params.gain.smoothed.next();
            let drive = self.params.drive.smoothed.next();
            let lfo_rate = self.params.lfo_rate.smoothed.next();
            let vp = VoiceParams {
                osc1_wave: self.params.osc1_wave.value(),
                osc2_wave: self.params.osc2_wave.value(),
                osc_mix: self.params.osc_mix.smoothed.next(),
                osc2_detune: self.params.osc2_detune.smoothed.next(),
                osc2_octave: self.params.osc2_octave.value(),
                fm_amount: self.params.fm_amount.smoothed.next(),
                cutoff: self.params.cutoff.smoothed.next(),
                resonance: self.params.resonance.smoothed.next(),
                filt_env_oct: self.params.filt_env_amount.smoothed.next(),
                lfo_oct: self.params.lfo_depth.smoothed.next(),
                amp_a: self.params.amp_attack.value(),
                amp_d: self.params.amp_decay.value(),
                amp_s: self.params.amp_sustain.value(),
                amp_r: self.params.amp_release.value(),
                flt_a: self.params.flt_attack.value(),
                flt_d: self.params.flt_decay.value(),
                flt_s: self.params.flt_sustain.value(),
                flt_r: self.params.flt_release.value(),
                sr,
            };

            // Global LFO (shared across voices) — drives the wobble.
            let lfo_value = naive_wave(self.params.lfo_wave.value(), self.lfo_phase);
            self.lfo_phase = (self.lfo_phase + lfo_rate / sr).fract();

            let mut sum = 0.0;
            for v in self.voices.iter_mut() {
                sum += v.render(&vp, lfo_value);
            }

            // Soft-clip (drive) then output gain. tanh keeps it bounded to ±1.
            let out = (sum * (1.0 + drive * 24.0)).tanh() * gain;

            for sample in channel_samples {
                *sample = out;
            }
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for LanternSynth {
    const CLAP_ID: &'static str = "com.alva.lantern-synth";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Subtractive synth with FM cross-mod, built for dubstep bass");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::Instrument,
        ClapFeature::Synthesizer,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for LanternSynth {
    // Must be exactly 16 bytes and unique to this plugin.
    const VST3_CLASS_ID: [u8; 16] = *b"LanternSynthAlva";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Instrument, Vst3SubCategory::Synth];
}

nih_export_clap!(LanternSynth);
nih_export_vst3!(LanternSynth);
