//! Radix-2 iterative FFT (Cooley-Tukey), precomputed tables, allocation-free
//! after construction. Feeds the spectrum analyzer; ours, no deps.

pub struct Fft {
    n: usize,
    rev: Vec<u32>,
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Fft {
    /// `n` must be a power of two.
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two());
        let bits = n.trailing_zeros();
        let rev = (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect();
        let (cos, sin) = (0..n / 2)
            .map(|k| {
                let a = -std::f32::consts::TAU * k as f32 / n as f32;
                (a.cos(), a.sin())
            })
            .unzip();
        Self { n, rev, cos, sin }
    }

    /// In-place forward transform of (re, im), both of length `n`.
    pub fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        debug_assert!(re.len() == n && im.len() == n);

        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }

        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            let mut base = 0;
            while base < n {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let (i1, i2) = (base + k, base + k + half);
                    let tr = re[i2] * wr - im[i2] * wi;
                    let ti = re[i2] * wi + im[i2] * wr;
                    re[i2] = re[i1] - tr;
                    im[i2] = im[i1] - ti;
                    re[i1] += tr;
                    im[i1] += ti;
                }
                base += len;
            }
            len *= 2;
        }
    }
}
