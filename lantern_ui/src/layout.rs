//! Grid layout: hand out cells derived from one area, so widgets align by
//! construction instead of by arithmetic. Pair with `Rect`'s combinators
//! (`col`/`row`/`strip_*`/`centered`) for everything a face needs.

use lantern_gpu::Rect;

/// An `area` divided into equal columns and rows with gutters. `cell` and
/// `span` return rects; two cells in the same row share top and bottom
/// edges exactly, whatever lives inside them.
pub struct Grid {
    area: Rect,
    cols: u32,
    rows: u32,
    gutter_x: f32,
    gutter_y: f32,
}

impl Grid {
    pub fn new(area: Rect, cols: u32, rows: u32, gutter: f32) -> Self {
        Self {
            area,
            cols: cols.max(1),
            rows: rows.max(1),
            gutter_x: gutter,
            gutter_y: gutter,
        }
    }

    /// Different horizontal/vertical gutters.
    pub fn gutters(mut self, gutter_x: f32, gutter_y: f32) -> Self {
        self.gutter_x = gutter_x;
        self.gutter_y = gutter_y;
        self
    }

    pub fn cell(&self, col: u32, row: u32) -> Rect {
        self.span(col, col, row, row)
    }

    /// Rect covering columns `c0..=c1` and rows `r0..=r1` (inclusive),
    /// gutters between spanned cells included.
    pub fn span(&self, c0: u32, c1: u32, r0: u32, r1: u32) -> Rect {
        let cw = (self.area.w - self.gutter_x * (self.cols - 1) as f32) / self.cols as f32;
        let rh = (self.area.h - self.gutter_y * (self.rows - 1) as f32) / self.rows as f32;
        Rect::new(
            self.area.x + c0 as f32 * (cw + self.gutter_x),
            self.area.y + r0 as f32 * (rh + self.gutter_y),
            (c1 - c0 + 1) as f32 * cw + (c1 - c0) as f32 * self.gutter_x,
            (r1 - r0 + 1) as f32 * rh + (r1 - r0) as f32 * self.gutter_y,
        )
    }
}
