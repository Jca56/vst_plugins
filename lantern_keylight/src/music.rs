//! Note names, scale definitions, and interval helpers for the cheat sheet.

pub const NOTE_NAMES: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];

pub struct Scale {
    pub name: &'static str,
    /// Bit i set = interval i (semitones above the root) is in the scale.
    pub mask: u16,
}

impl Scale {
    pub fn contains(&self, interval: u8) -> bool {
        self.mask >> (interval % 12) & 1 == 1
    }
}

const fn mask(intervals: &[u8]) -> u16 {
    let mut m = 0u16;
    let mut i = 0;
    while i < intervals.len() {
        m |= 1 << intervals[i];
        i += 1;
    }
    m
}

/// Twelve scales, laid out on the face as two columns of six.
/// Order is persisted in projects via the Scale param — append only.
pub const SCALES: &[Scale] = &[
    Scale { name: "Major", mask: mask(&[0, 2, 4, 5, 7, 9, 11]) },
    Scale { name: "Natural Minor", mask: mask(&[0, 2, 3, 5, 7, 8, 10]) },
    Scale { name: "Harmonic Minor", mask: mask(&[0, 2, 3, 5, 7, 8, 11]) },
    Scale { name: "Melodic Minor", mask: mask(&[0, 2, 3, 5, 7, 9, 11]) },
    Scale { name: "Major Pent.", mask: mask(&[0, 2, 4, 7, 9]) },
    Scale { name: "Minor Pent.", mask: mask(&[0, 3, 5, 7, 10]) },
    Scale { name: "Blues", mask: mask(&[0, 3, 5, 6, 7, 10]) },
    Scale { name: "Dorian", mask: mask(&[0, 2, 3, 5, 7, 9, 10]) },
    Scale { name: "Phrygian", mask: mask(&[0, 1, 3, 5, 7, 8, 10]) },
    Scale { name: "Phrygian Dom.", mask: mask(&[0, 1, 4, 5, 7, 8, 10]) },
    Scale { name: "Mixolydian", mask: mask(&[0, 2, 4, 5, 7, 9, 10]) },
    Scale { name: "Lydian", mask: mask(&[0, 2, 4, 6, 7, 9, 11]) },
];

/// Interval (semitones above the root) -> cheat-sheet degree name.
pub fn degree_name(interval: usize) -> &'static str {
    [
        "the root", "the ♭2", "the 2nd", "the ♭3", "the 3rd", "the 4th",
        "the ♭5", "the 5th", "the ♭6", "the 6th", "the ♭7", "the 7th",
    ][interval % 12]
}
