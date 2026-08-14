//! The Lantern synth voice: ported from the original nih-plug-era Lantern,
//! now true stereo. Per voice: Osc1 + Osc2 with FM cross-mod (osc2
//! phase-modulates osc1), unison copies panned across the field -> stereo
//! mix -> per-channel TPT state-variable filter / comb, cutoff driven by a
//! filter envelope and the global LFO -> amp envelope * velocity.
//! Dependency-free.

use std::f32::consts::{PI, TAU};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Waveform {
    Sine,
    Triangle,
    Saw,
    Square,
    /// 25% pulse — nasal, hollow.
    Pulse,
    /// 12.5% pulse — thin, buzzy.
    Narrow,
    /// Shark fin: fast rise, long fall. Between triangle and saw.
    Shark,
    /// White noise (ignores pitch).
    Noise,
}

impl Waveform {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Sine,
            1 => Self::Triangle,
            2 => Self::Saw,
            3 => Self::Square,
            4 => Self::Pulse,
            5 => Self::Narrow,
            6 => Self::Shark,
            _ => Self::Noise,
        }
    }
}

/// Maximum unison voices per oscillator.
pub const MAX_UNISON: usize = 7;

/// One oscillator's per-block settings. `ratios` carries the frequency
/// ratio of each unison copy with pitch and detune spread baked in, and
/// `gains` its (left, right) pan gains with the stereo width baked in, so
/// the per-sample path never touches powf or trig.
#[derive(Clone, Copy)]
pub struct OscSettings {
    pub enabled: bool,
    pub wave: Waveform,
    pub volume: f32,
    pub voices: usize,
    pub ratios: [f32; MAX_UNISON],
    pub gains: [(f32, f32); MAX_UNISON],
}

/// Per-sample scalar snapshot of everything a voice needs.
pub struct VoiceParams {
    pub osc: [OscSettings; 2],
    pub fm_amount: f32,
    pub flt_on: bool,
    pub flt_mode: FilterMode,
    /// Comb modes only: tune the comb to the played note (the cutoff knob
    /// becomes an offset around it — knob center = the note itself).
    pub keytrack: bool,
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
    rng: u32,
    pub amp_env: Adsr,
    pub flt_env: Adsr,
    filter: [Svf; 2],
    comb: [Comb; 2],
}

impl Voice {
    pub fn new() -> Self {
        Self {
            active: false,
            note: 0,
            velocity: 0.0,
            phases: [[0.0; MAX_UNISON]; 2],
            rng: 0x2F6E_2B1F,
            amp_env: Adsr::new(),
            flt_env: Adsr::new(),
            filter: [Svf::new(), Svf::new()],
            comb: [Comb::new(), Comb::new()],
        }
    }

    /// Size the combs' delay lines for this sample rate. Must run before
    /// processing; a comb with no buffer passes audio through dry.
    pub fn prepare(&mut self, sr: f32) {
        for c in &mut self.comb {
            c.alloc(sr);
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
        for f in &mut self.filter {
            f.reset();
        }
        for c in &mut self.comb {
            c.reset();
        }
        self.amp_env.trigger();
        self.flt_env.trigger();
    }

    pub fn note_off(&mut self) {
        self.amp_env.release();
        self.flt_env.release();
    }

    /// One oscillator: its unison copies panned across the field by their
    /// baked-in gains, all phase-modulated together by `pm`. Returns
    /// (left, right, mono) — the mono sum feeds FM so the modulator stays
    /// image-stable however wide the spread.
    fn run_osc(
        &mut self,
        oi: usize,
        set: &OscSettings,
        base: f32,
        sr: f32,
        pm: f32,
    ) -> (f32, f32, f32) {
        if !set.enabled {
            return (0.0, 0.0, 0.0);
        }
        let n = set.voices.clamp(1, MAX_UNISON);
        let (mut l, mut r, mut m) = (0.0, 0.0, 0.0);
        for k in 0..n {
            let dt = (base * set.ratios[k] / sr).min(0.49);
            let ph = (self.phases[oi][k] + pm).rem_euclid(1.0);
            let s = osc(set.wave, ph, dt, &mut self.rng);
            let (gl, gr) = set.gains[k];
            l += s * gl;
            r += s * gr;
            m += s;
            self.phases[oi][k] = (self.phases[oi][k] + dt).fract();
        }
        let norm = (n as f32).sqrt();
        (l / norm, r / norm, m / norm)
    }

    pub fn render(&mut self, p: &VoiceParams, lfo_value: f32) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }

        let amp = self.amp_env.process(p.amp_a, p.amp_d, p.amp_s, p.amp_r, p.sr);
        let fenv = self.flt_env.process(p.flt_a, p.flt_d, p.flt_s, p.flt_r, p.sr);
        // The amp envelope owns the voice's lifetime.
        if self.amp_env.stage == EnvStage::Idle {
            self.active = false;
            return (0.0, 0.0);
        }

        let base = midi_to_freq(self.note);
        // Osc 2 first: its mono sum phase-modulates Osc 1 (the "FM").
        let (l2, r2, m2) = self.run_osc(1, &p.osc[1], base, p.sr, 0.0);
        let (l1, r1, _) = self.run_osc(0, &p.osc[0], base, p.sr, m2 * p.fm_amount);
        let mixed = [
            l1 * p.osc[0].volume + l2 * p.osc[1].volume,
            r1 * p.osc[0].volume + r2 * p.osc[1].volume,
        ];

        // Cutoff modulated in octaves (musical) by filter env + global LFO;
        // both channels share the sweep, each keeps its own filter state.
        let filtered = if p.flt_on {
            let mod_oct = p.filt_env_oct * fenv + p.lfo_oct * lfo_value;
            match p.flt_mode {
                FilterMode::CombPlus | FilterMode::CombMinus => {
                    // Comb pitch: the cutoff knob, or — keytracked — the
                    // note itself with the knob as an offset (632 Hz, the
                    // knob's center, = played pitch). Env + wobble sweep it
                    // in octaves, same as the SVF's cutoff.
                    let tuned = if p.keytrack {
                        base * (p.cutoff / 632.45)
                    } else {
                        p.cutoff
                    };
                    let f = (tuned * 2.0f32.powf(mod_oct)).clamp(25.0, p.sr * 0.45);
                    let g = 0.98 * p.resonance.clamp(0.0, 1.0);
                    let g = if p.flt_mode == FilterMode::CombMinus { -g } else { g };
                    // Feedback comb rings up to 1/(1-g); pull it back so
                    // high resonance stays hot but not obliterating.
                    let damp = 1.0 - 0.55 * g.abs();
                    let mut out = [0.0f32; 2];
                    for ch in 0..2 {
                        out[ch] = self.comb[ch].process(mixed[ch], p.sr / f, g) * damp;
                    }
                    out
                }
                _ => {
                    let cutoff = (p.cutoff * 2.0f32.powf(mod_oct)).clamp(20.0, p.sr * 0.45);
                    let mut out = [0.0f32; 2];
                    for ch in 0..2 {
                        let (lp, bp, hp) =
                            self.filter[ch].process(mixed[ch], cutoff, p.resonance, p.sr);
                        out[ch] = match p.flt_mode {
                            FilterMode::Lowpass => lp,
                            FilterMode::Bandpass => bp,
                            FilterMode::Highpass => hp,
                            _ => lp + hp,
                        };
                    }
                    out
                }
            }
        } else {
            mixed
        };

        let v = amp * self.velocity;
        (filtered[0] * v, filtered[1] * v)
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

    /// One TPT step; returns (lowpass, bandpass, highpass) — the SVF gives
    /// all three at once, `FilterMode` just picks.
    pub fn process(&mut self, v0: f32, cutoff: f32, res: f32, sr: f32) -> (f32, f32, f32) {
        let g = (PI * cutoff / sr).tan();
        // res in [0,1] -> k = 1/Q from 2 (gentle) down to ~0.02 (screaming).
        let k = svf_k(res);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = v0 - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        let hp = v0 - k * v1 - v2;
        (v2, v1, hp)
    }
}

/// Resonance knob -> the SVF's damping k (shared with the face's response
/// curve so drawn and heard agree).
pub fn svf_k(res: f32) -> f32 {
    (2.0 - 1.98 * res).max(0.02)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FilterMode {
    Lowpass,
    Bandpass,
    Highpass,
    Notch,
    /// Feedback comb, positive: peaks on the harmonic series — metallic,
    /// stringy resonance (Serum's Cmb+ flavor).
    CombPlus,
    /// Feedback comb, negative: peaks between the harmonics — hollow,
    /// vowely, the talking-bass one.
    CombMinus,
}

impl FilterMode {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Lowpass,
            1 => Self::Bandpass,
            2 => Self::Highpass,
            3 => Self::Notch,
            4 => Self::CombPlus,
            _ => Self::CombMinus,
        }
    }
}

// --- Feedback comb: y[n] = x[n] + g * y[n - D], fractional D ---

/// One tuned delay line with feedback. `D = sr / freq` puts resonant peaks
/// at every multiple of `freq` (g > 0) or halfway between them (g < 0) —
/// keytracked, that's a played-pitch resonator ringing over the oscillators.
pub struct Comb {
    buf: Vec<f32>,
    w: usize,
}

impl Comb {
    pub fn new() -> Self {
        Self { buf: Vec::new(), w: 0 }
    }

    /// Size for the lowest comb pitch (25 Hz) at this sample rate.
    pub fn alloc(&mut self, sr: f32) {
        let n = (sr / 25.0) as usize + 8;
        if self.buf.len() != n {
            self.buf = vec![0.0; n];
        }
        self.w = 0;
    }

    pub fn reset(&mut self) {
        self.buf.iter_mut().for_each(|x| *x = 0.0);
        self.w = 0;
    }

    /// One sample through the comb; `delay` in samples (fractional, linear
    /// interp). Unprepared combs pass through dry.
    pub fn process(&mut self, x: f32, delay: f32, g: f32) -> f32 {
        let n = self.buf.len();
        if n < 8 {
            return x;
        }
        let d = delay.clamp(2.0, (n - 3) as f32);
        let rp = self.w as f32 - d + n as f32;
        let i0 = rp as usize;
        let frac = rp - i0 as f32;
        let s0 = self.buf[i0 % n];
        let s1 = self.buf[(i0 + 1) % n];
        let y = x + g * (s0 + (s1 - s0) * frac);
        self.buf[self.w] = y;
        self.w = (self.w + 1) % n;
        y
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

/// PolyBLEP pulse at an arbitrary duty cycle.
fn pulse(phase: f32, dt: f32, duty: f32) -> f32 {
    let naive = if phase < duty { 1.0 } else { -1.0 };
    // DC-compensate so thin pulses stay centered.
    let centered = naive - (2.0 * duty - 1.0);
    centered + poly_blep(phase, dt) - poly_blep((phase + (1.0 - duty)).fract(), dt)
}

/// Band-limited oscillator (audio rate). `dt` is the per-sample phase step;
/// `rng` feeds the noise shape.
pub fn osc(wave: Waveform, phase: f32, dt: f32, rng: &mut u32) -> f32 {
    match wave {
        Waveform::Sine => (phase * TAU).sin(),
        Waveform::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
        Waveform::Saw => (2.0 * phase - 1.0) - poly_blep(phase, dt),
        Waveform::Square => pulse(phase, dt, 0.5),
        Waveform::Pulse => pulse(phase, dt, 0.25),
        Waveform::Narrow => pulse(phase, dt, 0.125),
        Waveform::Shark => {
            // Rise over the first quarter, fall over the rest: continuous,
            // so only mild slope-discontinuity aliasing (like triangle).
            if phase < 0.25 {
                -1.0 + 8.0 * phase
            } else {
                1.0 - (phase - 0.25) / 0.75 * 2.0
            }
        }
        Waveform::Noise => {
            *rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (*rng >> 8) as f32 / 8_388_608.0 - 1.0
        }
    }
}

/// Naive shape — fine for the sub-audio LFO (first four shapes only; the
/// rest degrade to their nearest classic).
pub fn naive_wave(wave: Waveform, phase: f32) -> f32 {
    match wave {
        Waveform::Sine => (phase * TAU).sin(),
        Waveform::Triangle | Waveform::Shark => 1.0 - 4.0 * (phase - 0.5).abs(),
        Waveform::Saw => 2.0 * phase - 1.0,
        _ => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
    }
}
