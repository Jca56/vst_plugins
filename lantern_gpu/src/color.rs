#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: srgb_to_linear(r as f32 / 255.0),
            g: srgb_to_linear(g as f32 / 255.0),
            b: srgb_to_linear(b as f32 / 255.0),
            a: a as f32 / 255.0,
        }
    }

    pub fn from_rgb8(r: u8, g: u8, b: u8) -> Self {
        Self::from_rgba8(r, g, b, 255)
    }

    pub fn from_srgb(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self {
            r: srgb_to_linear(r),
            g: srgb_to_linear(g),
            b: srgb_to_linear(b),
            a,
        }
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Parse a hex color string: `"#RGB"`, `"#RRGGBB"`, or `"#RRGGBBAA"`.
    /// The `#` prefix is optional. Returns `None` on invalid input.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
                Some(Self::from_rgb8(r << 4 | r, g << 4 | g, b << 4 | b))
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Self::from_rgb8(r, g, b))
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(Self::from_rgba8(r, g, b, a))
            }
            _ => None,
        }
    }

    /// Linearly interpolate between `self` and `other` by `t` (0.0–1.0).
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
        }
    }

    /// Make the color lighter by `amount` (0.0–1.0). Blends toward white.
    pub fn lighten(self, amount: f32) -> Self {
        self.lerp(Self::rgb(1.0, 1.0, 1.0), amount).with_alpha(self.a)
    }

    /// Make the color darker by `amount` (0.0–1.0). Blends toward black.
    pub fn darken(self, amount: f32) -> Self {
        self.lerp(Self::rgb(0.0, 0.0, 0.0), amount).with_alpha(self.a)
    }

    /// Adjust saturation. `factor` < 1.0 desaturates, > 1.0 saturates.
    pub fn saturate(self, factor: f32) -> Self {
        let lum = 0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b;
        Self {
            r: lum + (self.r - lum) * factor,
            g: lum + (self.g - lum) * factor,
            b: lum + (self.b - lum) * factor,
            a: self.a,
        }
    }

    /// Convert from linear float back to sRGB 8-bit [r, g, b, a].
    pub fn to_srgb8(self) -> [u8; 4] {
        [
            (linear_to_srgb(self.r.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8,
            (linear_to_srgb(self.g.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8,
            (linear_to_srgb(self.b.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8,
            (self.a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        ]
    }

    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    pub const TRANSPARENT: Self = Self::rgba(0.0, 0.0, 0.0, 0.0);
}

pub(crate) fn srgb_to_linear(s: f32) -> f32 {
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

pub(crate) fn linear_to_srgb(l: f32) -> f32 {
    if l <= 0.0031308 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}
