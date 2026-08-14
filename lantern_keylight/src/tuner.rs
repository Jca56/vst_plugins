//! Monophonic pitch detection: YIN (de Cheveigné & Kawahara 2002) on a
//! decimated mono tap of the output. Decimation buys bass reach cheaply —
//! at ~12 kHz analysis rate, 600 lags reach below 21 Hz, and the whole
//! difference function costs ~600k multiply-adds per ~43 ms hop.
//!
//! Everything is allocated in `setup`; `feed` and `analyze` run on the
//! audio thread allocation-free.

/// Integration window (decimated samples).
const W: usize = 1024;
/// Highest lag searched (decimated samples) — sets the lowest detectable Hz.
const TMAX: usize = 600;
/// Ring size (power of two, >= W + TMAX).
const RING: usize = 2048;
/// Decimated samples between analyses (~43 ms at 12 kHz).
const HOP: usize = 512;
/// CMND acceptance threshold (standard YIN operating point).
const THRESHOLD: f32 = 0.15;

pub struct PitchDetector {
    /// Decimation factor (input samples per analysis sample).
    decim: usize,
    /// Decimated (analysis) sample rate.
    rate: f32,
    acc: f32,
    acc_n: usize,
    ring: Vec<f32>,
    widx: usize,
    since_hop: usize,
    /// Linearized latest W + TMAX samples, oldest first.
    scratch: Vec<f32>,
    /// Difference function -> cumulative-mean-normalized in place.
    diff: Vec<f32>,
    freq: f32,
    clarity: f32,
}

impl PitchDetector {
    pub fn new() -> Self {
        Self {
            decim: 4,
            rate: 12_000.0,
            acc: 0.0,
            acc_n: 0,
            ring: Vec::new(),
            widx: 0,
            since_hop: 0,
            scratch: Vec::new(),
            diff: Vec::new(),
            freq: 0.0,
            clarity: 0.0,
        }
    }

    pub fn setup(&mut self, sample_rate: f64) {
        self.decim = ((sample_rate / 12_000.0).round() as usize).max(1);
        self.rate = (sample_rate / self.decim as f64) as f32;
        self.ring = vec![0.0; RING];
        self.scratch = vec![0.0; W + TMAX];
        self.diff = vec![0.0; TMAX];
        self.reset();
    }

    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.acc = 0.0;
        self.acc_n = 0;
        self.widx = 0;
        self.since_hop = 0;
        self.freq = 0.0;
        self.clarity = 0.0;
    }

    /// Detected fundamental in Hz (0.0 until something has been heard).
    pub fn freq(&self) -> f32 {
        self.freq
    }

    /// 0..1 confidence; below ~0.5 the display should read "no pitch".
    pub fn clarity(&self) -> f32 {
        self.clarity
    }

    /// Feed one mono sample at the host sample rate.
    #[inline]
    pub fn feed(&mut self, s: f32) {
        self.acc += s;
        self.acc_n += 1;
        if self.acc_n >= self.decim {
            // Boxcar average = crude anti-alias; plenty for pitch tracking.
            let v = self.acc / self.decim as f32;
            self.acc = 0.0;
            self.acc_n = 0;
            self.ring[self.widx] = v;
            self.widx = (self.widx + 1) & (RING - 1);
            self.since_hop += 1;
            if self.since_hop >= HOP {
                self.since_hop = 0;
                self.analyze();
            }
        }
    }

    fn analyze(&mut self) {
        let n = W + TMAX;
        for i in 0..n {
            self.scratch[i] = self.ring[(self.widx + RING - n + i) & (RING - 1)];
        }

        // Level gate: don't chase pitches in silence (< about -80 dBFS RMS).
        let mut energy = 0.0f32;
        for &x in &self.scratch[..W] {
            energy += x * x;
        }
        if energy / W as f32 <= 1e-8 {
            self.clarity = 0.0;
            return;
        }

        // Difference function d(tau), then CMND in place.
        for tau in 1..TMAX {
            let mut sum = 0.0f32;
            for i in 0..W {
                let d = self.scratch[i] - self.scratch[i + tau];
                sum += d * d;
            }
            self.diff[tau] = sum;
        }
        self.diff[0] = 1.0;
        let mut cum = 0.0f32;
        for tau in 1..TMAX {
            cum += self.diff[tau];
            self.diff[tau] = if cum > 0.0 {
                self.diff[tau] * tau as f32 / cum
            } else {
                1.0
            };
        }

        // First dip below threshold, walked to its local minimum; fall back
        // to the global minimum if it's convincing enough.
        let mut tau = 0usize;
        for t in 2..TMAX - 1 {
            if self.diff[t] < THRESHOLD {
                let mut t = t;
                while t + 1 < TMAX - 1 && self.diff[t + 1] < self.diff[t] {
                    t += 1;
                }
                tau = t;
                break;
            }
        }
        if tau == 0 {
            let mut best = (0usize, f32::MAX);
            for t in 2..TMAX - 1 {
                if self.diff[t] < best.1 {
                    best = (t, self.diff[t]);
                }
            }
            if best.1 < 0.35 {
                tau = best.0;
            } else {
                self.clarity = 0.0;
                return;
            }
        }

        // Parabolic interpolation for a sub-sample period estimate.
        let (y1, y2, y3) = (self.diff[tau - 1], self.diff[tau], self.diff[tau + 1]);
        let denom = y1 - 2.0 * y2 + y3;
        let delta = if denom.abs() > 1e-12 {
            (0.5 * (y1 - y3) / denom).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let f = self.rate / (tau as f32 + delta);
        if !(16.0..=4000.0).contains(&f) {
            self.clarity = 0.0;
            return;
        }
        self.freq = f;
        self.clarity = (1.0 - self.diff[tau]).clamp(0.0, 1.0);
    }
}
