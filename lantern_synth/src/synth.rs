//! The Lantern synth voice: ported intact from the original nih-plug-era
//! Lantern. Per voice: Osc1 + Osc2 with FM cross-mod (osc2 phase-modulates
//! osc1) -> mix -> TPT state-variable low-pass, cutoff driven by a filter
//! envelope and the global LFO -> amp envelope * velocity. Dependency-free.

use std::f32::consts::{PI, TAU};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Waveform {
    Sine,
    Triangle,
    Saw,
    Square,
}

impl Waveform {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Sine,
            1 => Self::Triangle,
            2 => Self::Saw,
            _ => Self::Square,
        }
    }
}

/// Per-sample scalar snapshot of everything a voice needs.
pub struct VoiceParams {
    pub osc1_wave: Waveform,
    pub osc2_wave: Waveform,
    pub osc_mix: f32,
    pub osc2_detune: f32,
    pub osc2_octave: i32,
    pub fm_amount: f32,
    pub cutoff: f32,
    pub resonance: f32,
    pub filt_env_oct: f32,
    pub lfo_oct: f32,
    pub amp_a: f32,
    pub amp_d: f32,
    pub amp_s: f32,
    pub amp_r: f32,
    pub flt_a: f32,
    pub flt_d: f32,
    pub flt_s: f32,
    pub flt_r: f32,
    pub sr: f32,
}

pub struct Voice {
    pub active: bool,
    pub note: u8,
    pub velocity: f32,
    phase1: f32,
    phase2: f32,
    pub amp_env: Adsr,
    pub flt_env: Adsr,
    filter: Svf,
}

impl Voice {
    pub fn new() -> Self {
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

    pub fn note_on(&mut self, note: u8, velocity: f32) {
        self.active = true;
        self.note = note;
        self.velocity = velocity;
        self.phase1 = 0.0;
        self.phase2 = 0.0;
        self.filter.reset();
        self.amp_env.trigger();
        self.flt_env.trigger();
    }

    pub fn note_off(&mut self) {
        self.amp_env.release();
        self.flt_env.release();
    }

    pub fn render(&mut self, p: &VoiceParams, lfo_value: f32) -> f32 {
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

        // Cutoff modulated in octaves (musical) by filter env + global LFO.
        let mod_oct = p.filt_env_oct * fenv + p.lfo_oct * lfo_value;
        let cutoff = (p.cutoff * 2.0f32.powf(mod_oct)).clamp(20.0, p.sr * 0.45);
        let filtered = self.filter.process_lp(mixed, cutoff, p.resonance, p.sr);

        filtered * amp * self.velocity
    }
}

// --- ADSR envelope (linear segments) ---

#[derive(Clone, Copy, PartialEq)]
pub enum EnvStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Copy)]
pub struct Adsr {
    pub stage: EnvStage,
    pub level: f32,
}

impl Adsr {
    pub fn new() -> Self {
        Self {
            stage: EnvStage::Idle,
            level: 0.0,
        }
    }

    pub fn trigger(&mut self) {
        self.stage = EnvStage::Attack;
    }

    pub fn release(&mut self) {
        if self.stage != EnvStage::Idle {
            self.stage = EnvStage::Release;
        }
    }

    pub fn process(&mut self, a: f32, d: f32, s: f32, r: f32, sr: f32) -> f32 {
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
pub struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    pub fn new() -> Self {
        Self { ic1: 0.0, ic2: 0.0 }
    }

    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    pub fn process_lp(&mut self, v0: f32, cutoff: f32, res: f32, sr: f32) -> f32 {
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

pub fn midi_to_freq(note: u8) -> f32 {
    440.0 * 2.0f32.powf((note as f32 - 69.0) / 12.0)
}

/// PolyBLEP correction band-limiting the discontinuities in saw/square.
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

/// Band-limited oscillator (audio rate). `dt` is the per-sample phase step.
pub fn osc(wave: Waveform, phase: f32, dt: f32) -> f32 {
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

/// Naive shape — fine for the sub-audio LFO.
pub fn naive_wave(wave: Waveform, phase: f32) -> f32 {
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
