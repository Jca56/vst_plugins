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

/// Maximum unison voices per oscillator.
pub const MAX_UNISON: usize = 7;

/// One oscillator's per-block settings. `ratios` carries the frequency
/// ratio of each unison copy with pitch and detune spread baked in, so the
/// per-sample path never touches powf.
#[derive(Clone, Copy)]
pub struct OscSettings {
    pub enabled: bool,
    pub wave: Waveform,
    pub volume: f32,
    pub voices: usize,
    pub ratios: [f32; MAX_UNISON],
}

/// Per-sample scalar snapshot of everything a voice needs.
pub struct VoiceParams {
    pub osc: [OscSettings; 2],
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
    phases: [[f32; MAX_UNISON]; 2],
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
            phases: [[0.0; MAX_UNISON]; 2],
            amp_env: Adsr::new(),
            flt_env: Adsr::new(),
            filter: Svf::new(),
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: f32) {
        self.active = true;
        self.note = note;
        self.velocity = velocity;
        // Scatter unison phases deterministically (same note = same attack).
        for o in 0..2 {
            for k in 0..MAX_UNISON {
                let h = (note as u32)
                    .wrapping_mul(2_654_435_761)
                    .wrapping_add(((o * MAX_UNISON + k) as u32).wrapping_mul(40_503));
                self.phases[o][k] = (h >> 8) as f32 / 16_777_216.0;
            }
        }
        self.filter.reset();
        self.amp_env.trigger();
        self.flt_env.trigger();
    }

    pub fn note_off(&mut self) {
        self.amp_env.release();
        self.flt_env.release();
    }

    /// One oscillator: sum of its unison copies (equal-power normalized),
    /// all phase-modulated together by `pm`.
    fn run_osc(&mut self, oi: usize, set: &OscSettings, base: f32, sr: f32, pm: f32) -> f32 {
        if !set.enabled {
            return 0.0;
        }
        let n = set.voices.clamp(1, MAX_UNISON);
        let mut sum = 0.0;
        for k in 0..n {
            let dt = (base * set.ratios[k] / sr).min(0.49);
            let ph = (self.phases[oi][k] + pm).rem_euclid(1.0);
            sum += osc(set.wave, ph, dt);
            self.phases[oi][k] = (self.phases[oi][k] + dt).fract();
        }
        sum / (n as f32).sqrt()
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
        // Osc 2 first: it phase-modulates Osc 1 (the "FM").
        let s2 = self.run_osc(1, &p.osc[1], base, p.sr, 0.0);
        let s1 = self.run_osc(0, &p.osc[0], base, p.sr, s2 * p.fm_amount);
        let mixed = s1 * p.osc[0].volume + s2 * p.osc[1].volume;

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
