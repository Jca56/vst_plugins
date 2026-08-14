#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn center_x(&self) -> f32 {
        self.x + self.w * 0.5
    }

    pub fn center_y(&self) -> f32 {
        self.y + self.h * 0.5
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }

    pub fn expand(&self, amount: f32) -> Self {
        Self {
            x: self.x - amount,
            y: self.y - amount,
            w: self.w + amount * 2.0,
            h: self.h + amount * 2.0,
        }
    }

    pub fn translate(&self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..*self
        }
    }

    // ------------------------------------------------------------------
    // Layout combinators: derive rects from rects instead of counting
    // pixels. Widgets that share a parent share its edges by construction.
    // ------------------------------------------------------------------

    /// Shrink by `amount` on every side (negative expands).
    pub fn inset(&self, amount: f32) -> Self {
        self.expand(-amount)
    }

    /// A `w` x `h` rect centered inside this one.
    pub fn centered(&self, w: f32, h: f32) -> Self {
        Self::new(self.x + (self.w - w) * 0.5, self.y + (self.h - h) * 0.5, w, h)
    }

    /// The top `h` of this rect.
    pub fn strip_top(&self, h: f32) -> Self {
        Self::new(self.x, self.y, self.w, h)
    }

    /// The bottom `h` of this rect.
    pub fn strip_bottom(&self, h: f32) -> Self {
        Self::new(self.x, self.y + self.h - h, self.w, h)
    }

    /// The left `w` of this rect.
    pub fn strip_left(&self, w: f32) -> Self {
        Self::new(self.x, self.y, w, self.h)
    }

    /// The right `w` of this rect.
    pub fn strip_right(&self, w: f32) -> Self {
        Self::new(self.x + self.w - w, self.y, w, self.h)
    }

    /// Column `i` of `n` equal columns separated by `gutter`.
    pub fn col(&self, i: u32, n: u32, gutter: f32) -> Self {
        let n = n.max(1);
        let w = (self.w - gutter * (n - 1) as f32) / n as f32;
        Self::new(self.x + i as f32 * (w + gutter), self.y, w, self.h)
    }

    /// Row `i` of `n` equal rows separated by `gutter`.
    pub fn row(&self, i: u32, n: u32, gutter: f32) -> Self {
        let n = n.max(1);
        let h = (self.h - gutter * (n - 1) as f32) / n as f32;
        Self::new(self.x, self.y + i as f32 * (h + gutter), self.w, h)
    }

    /// A rect spanning from this rect's top edge to `other`'s bottom edge,
    /// horizontally as this one — for hanging a column on shared lines.
    pub fn spanning_to(&self, other: &Rect) -> Self {
        Self::new(self.x, self.y, self.w, other.y + other.h - self.y)
    }

    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = (self.x + self.w).min(other.x + other.w);
        let b = (self.y + self.h).min(other.y + other.h);
        if r > x && b > y {
            Some(Rect::new(x, y, r - x, b - y))
        } else {
            None
        }
    }
}
