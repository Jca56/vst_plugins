//! Two-tap crossfaded delay-line pitch shifter, pitch-synchronized.
//!
//! The naive granular shifter detunes: every crossfade drags the output
//! phase toward the incoming tap, and unless the taps sit an exact number
//! of input periods apart that drag accumulates into a systematic pitch
//! error of up to the grain rate in Hz. So Keylight's shifter is period-
//! locked (PSOLA style): the DSP feeds it the input period from a YIN
//! detector, the window snaps to an even multiple of it, and the tap
//! spacing (window/2) becomes a whole number of periods — splices land
//! phase-aligned and the drift cancels exactly. Unpitched input falls back
//! to the nominal window, i.e. ordinary granular behavior.
//!
//! Taps store their delays directly and wrap by re-spacing against the
//! other tap at their own zero-gain moment, so window retunes never jump a
//! tap mid-grain: the window glides and only the gain envelopes reshape.

use std::f32::consts::PI;

/// Nominal grain window in seconds (used when the input isn't pitched).
const WINDOW_S: f32 = 0.040;
/// Longest window we'll lock to: two periods of ~24 Hz.
const WINDOW_MAX_S: f32 = 0.090;

pub struct PitchShifter {
    left: Vec<f32>,
    right: Vec<f32>,
    mask: usize,
    write: usize,
    /// Tap delays in samples, kept ~window/2 apart.
    d_a: f32,
    d_b: f32,
    /// Current (gliding) and target window length in samples.
    window: f32,
    window_target: f32,
    nominal: f32,
    coef_w: f32,
}

impl PitchShifter {
    pub fn new() -> Self {
        Self {
            left: Vec::new(),
            right: Vec::new(),
            mask: 0,
            write: 0,
            d_a: 0.0,
            d_b: 0.0,
            window: 1.0,
            window_target: 1.0,
            nominal: 1.0,
            coef_w: 0.0,
        }
    }

    pub fn setup(&mut self, sample_rate: f64) {
        let sr = sample_rate as f32;
        self.nominal = (sr * WINDOW_S).floor();
        let len = ((sr * WINDOW_MAX_S) as usize + 16).next_power_of_two();
        self.left = vec![0.0; len];
        self.right = vec![0.0; len];
        self.mask = len - 1;
        // ~30 ms window glide.
        self.coef_w = 1.0 - (-1.0 / (sr * 0.030)).exp();
        self.reset();
    }

    pub fn reset(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.write = 0;
        self.window = self.nominal;
        self.window_target = self.nominal;
        self.d_a = 0.0;
        self.d_b = self.nominal * 0.5;
    }

    /// Feed the detected input period in samples (None = unpitched): the
    /// window heads for the nearest even multiple of it.
    pub fn set_period(&mut self, period: Option<f32>) {
        self.window_target = match period {
            Some(p) if p > 8.0 => {
                let k = (self.nominal / (2.0 * p)).round().max(1.0);
                (2.0 * k * p).min(self.mask as f32 - 32.0)
            }
            _ => self.nominal,
        };
    }

    /// Push the next dry sample pair into the rings (call every sample,
    /// bypassed or not, so the buffer is warm when the shift engages).
    #[inline]
    pub fn push(&mut self, l: f32, r: f32) {
        self.write = (self.write + 1) & self.mask;
        self.left[self.write] = l;
        self.right[self.write] = r;
    }

    #[inline]
    fn tap(buf: &[f32], mask: usize, write: usize, delay: f32) -> f32 {
        // Base guard of 3 samples keeps the interpolator behind the write head.
        let pos = (write + 2 * (mask + 1)) as f32 - 3.0 - delay;
        let i0 = pos as usize;
        let frac = pos - i0 as f32;
        let a = buf[i0 & mask];
        let b = buf[(i0 + 1) & mask];
        a + (b - a) * frac
    }

    /// Advance the taps and return the crossfaded stereo output.
    /// `ratio` = 2^(semitones/12).
    #[inline]
    pub fn taps(&mut self, ratio: f32) -> (f32, f32) {
        self.window += (self.window_target - self.window) * self.coef_w;
        let w = self.window;

        // Both taps sweep at (1 - ratio); a tap crossing either end of the
        // window is at zero gain, so it silently re-spaces half a window
        // from its partner (which also self-heals spacing after retunes).
        let step = 1.0 - ratio;
        self.d_a += step;
        self.d_b += step;
        if self.d_a < 0.0 || self.d_a >= w {
            self.d_a = (self.d_b + 0.5 * w).rem_euclid(w);
        }
        if self.d_b < 0.0 || self.d_b >= w {
            self.d_b = (self.d_a + 0.5 * w).rem_euclid(w);
        }

        let ga = (PI * (self.d_a / w).clamp(0.0, 1.0)).sin();
        let gb = (PI * (self.d_b / w).clamp(0.0, 1.0)).sin();
        let l = ga * Self::tap(&self.left, self.mask, self.write, self.d_a)
            + gb * Self::tap(&self.left, self.mask, self.write, self.d_b);
        let r = ga * Self::tap(&self.right, self.mask, self.write, self.d_a)
            + gb * Self::tap(&self.right, self.mask, self.write, self.d_b);
        (l, r)
    }
}
