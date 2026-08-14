//! The Lantern look: one palette shared by every plugin so the family reads
//! as family.

use lantern_gpu::Color;

pub struct Theme {
    pub bg: Color,
    pub header_a: Color,
    pub header_b: Color,
    pub header_c: Color,
    pub panel: Color,
    pub panel_border: Color,
    pub well: Color,
    /// Deepest recess: tuner/meter wells, active edit boxes, pickers.
    pub well_deep: Color,
    pub track: Color,
    pub accent: Color,
    pub warm: Color,
    /// The cold counterpart to `warm`: deep blue, for highlights that
    /// should read as marked rather than lit (piano scale keys).
    pub cool: Color,
    pub needle: Color,
    pub text: Color,
    pub text_dim: Color,
    /// Level meter: healthy signal (0 dBFS and below).
    pub meter_ok: Color,
    /// Level meter: clipping (above 0 dBFS).
    pub meter_hot: Color,
}

impl Theme {
    /// The Lantern look, as Alva likes it: near-black ground, deep saturated
    /// gold accent, blue scale markings, bright chonky borders, big text —
    /// and the gold→orange fire divider as the family signature.
    pub fn lantern() -> Self {
        Self {
            // True neutral grays — no blue tint (Alva spotted the cool cast).
            bg: Color::from_rgb8(7, 7, 7),
            header_a: Color::from_rgb8(24, 18, 48),
            header_b: Color::from_rgb8(184, 132, 30),
            header_c: Color::from_rgb8(255, 111, 0),
            panel: Color::from_rgb8(18, 18, 18),
            panel_border: Color::from_rgb8(95, 95, 95),
            well: Color::from_rgb8(31, 31, 31),
            well_deep: Color::from_rgb8(13, 13, 13),
            track: Color::from_rgb8(49, 49, 49),
            accent: Color::from_rgb8(255, 166, 22),
            warm: Color::from_rgb8(255, 96, 28),
            cool: Color::from_rgb8(36, 108, 250),
            needle: Color::from_rgb8(242, 242, 242),
            text: Color::from_rgb8(242, 242, 242),
            text_dim: Color::from_rgb8(176, 176, 176),
            meter_ok: Color::from_rgb8(22, 155, 64),
            meter_hot: Color::from_rgb8(250, 45, 35),
        }
    }
}
