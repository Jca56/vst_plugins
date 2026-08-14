//! RBJ Audio-EQ-Cookbook biquads: coefficient recipes, a transposed
//! direct-form-II state, and an analytic magnitude response the face uses
//! to draw the curves — one source of truth for DSP and display.

use std::f32::consts::TAU;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    LowCut,
    LowShelf,
    Bell,
    HighShelf,
    HighCut,
}

#[derive(Clone, Copy, Default)]
pub struct Coeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

/// Cookbook coefficients. `gain_db` is ignored by the cut modes (Q shapes
/// their resonance instead).
pub fn coeffs(mode: Mode, f0: f32, gain_db: f32, q: f32, sr: f32) -> Coeffs {
    let f0 = f0.clamp(10.0, sr * 0.49);
    let q = q.max(0.05);
    let w = TAU * f0 / sr;
    let (sn, cs) = w.sin_cos();
    let alpha = sn / (2.0 * q);
    let a = 10f32.powf(gain_db / 40.0);

    let (b0, b1, b2, a0, a1, a2) = match mode {
        Mode::Bell => (
            1.0 + alpha * a,
            -2.0 * cs,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cs,
            1.0 - alpha / a,
        ),
        Mode::LowCut => {
            let b0 = (1.0 + cs) * 0.5;
            (b0, -(1.0 + cs), b0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha)
        }
        Mode::HighCut => {
            let b0 = (1.0 - cs) * 0.5;
            (b0, 1.0 - cs, b0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha)
        }
        Mode::LowShelf => {
            let sq = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) - (a - 1.0) * cs + sq),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - sq),
                (a + 1.0) + (a - 1.0) * cs + sq,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - sq,
            )
        }
        Mode::HighShelf => {
            let sq = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) + (a - 1.0) * cs + sq),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - sq),
                (a + 1.0) - (a - 1.0) * cs + sq,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - sq,
            )
        }
    };

    Coeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// Magnitude response in dB at frequency `f` — the curve the filter really
/// has, warping and all, straight from the coefficients. Complex evaluation
/// in f64: the all-real cosine form cancels catastrophically at low
/// frequencies where cos(w) is within f32 epsilon of 1.
pub fn mag_db(c: &Coeffs, f: f32, sr: f32) -> f32 {
    let w = std::f64::consts::TAU * (f as f64 / sr as f64).min(0.499);
    let (s1, c1) = w.sin_cos();
    let (s2, c2) = (2.0 * w).sin_cos();
    let (b0, b1, b2) = (c.b0 as f64, c.b1 as f64, c.b2 as f64);
    let (a1, a2) = (c.a1 as f64, c.a2 as f64);
    let nre = b0 + b1 * c1 + b2 * c2;
    let nim = b1 * s1 + b2 * s2;
    let dre = 1.0 + a1 * c1 + a2 * c2;
    let dim = a1 * s1 + a2 * s2;
    let num = nre * nre + nim * nim;
    let den = dre * dre + dim * dim;
    (10.0 * (num.max(1e-24) / den.max(1e-24)).log10()) as f32
}

/// Transposed direct-form II state (one channel).
#[derive(Clone, Copy, Default)]
pub struct BiquadState {
    z1: f32,
    z2: f32,
}

impl BiquadState {
    #[inline]
    pub fn process(&mut self, c: &Coeffs, x: f32) -> f32 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
