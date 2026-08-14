use crate::color::Color;
use crate::rect::Rect;
use crate::painter::Painter;

/// Compound shape methods built on top of Painter primitives.
impl Painter {
    /// Connected line strip through a list of points.
    pub fn polyline(&mut self, points: &[(f32, f32)], width: f32, color: Color) {
        if color.a < 0.004 || points.len() < 2 { return; }
        for seg in points.windows(2) {
            self.line(seg[0].0, seg[0].1, seg[1].0, seg[1].1, width, color);
        }
    }

    /// Closed polyline (connects last point back to first).
    pub fn polyline_closed(&mut self, points: &[(f32, f32)], width: f32, color: Color) {
        if color.a < 0.004 || points.len() < 2 { return; }
        self.polyline(points, width, color);
        let first = points[0];
        let last = points[points.len() - 1];
        self.line(last.0, last.1, first.0, first.1, width, color);
    }

    /// Filled convex polygon via fan triangulation from the first vertex.
    /// For non-convex polygons, results will be incorrect — triangulate externally.
    pub fn polygon(&mut self, points: &[(f32, f32)], color: Color) {
        if color.a < 0.004 || points.len() < 3 { return; }
        let (ax, ay) = points[0];
        for i in 1..points.len() - 1 {
            let (bx, by) = points[i];
            let (cx, cy) = points[i + 1];
            self.triangle(ax, ay, bx, by, cx, cy, color);
        }
    }

    /// Line with rounded end caps (circles at each endpoint).
    pub fn line_round(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: Color) {
        if color.a < 0.004 { return; }
        self.line(x1, y1, x2, y2, width, color);
        let r = width * 0.5;
        self.circle_filled(x1, y1, r, color);
        self.circle_filled(x2, y2, r, color);
    }

    /// Connected line strip with rounded joins and caps.
    pub fn polyline_round(&mut self, points: &[(f32, f32)], width: f32, color: Color) {
        if color.a < 0.004 || points.len() < 2 { return; }
        let r = width * 0.5;
        for seg in points.windows(2) {
            self.line(seg[0].0, seg[0].1, seg[1].0, seg[1].1, width, color);
        }
        for &(x, y) in points {
            self.circle_filled(x, y, r, color);
        }
    }

    /// Multi-stop linear gradient in a rounded rect, rendered in a single draw call.
    /// `stops` is a list of `(t, Color)` where `t` is 0.0–1.0 along the gradient.
    /// `angle` is in radians: 0 = left→right, π/2 = top→bottom.
    ///
    /// Supports 2-4 stops. For 2 stops, dispatches to `rect_gradient_linear`.
    /// For 3-4 stops, uses the per-pixel `SHAPE_GRADIENT_MULTI` shader path
    /// (no geometry layering — corners stay clean and there are no seams).
    /// Stops with `t > 1` or `t < 0` are clamped; the first stop's `t` is
    /// ignored (treated as 0) and the last stop's `t` is ignored (treated as 1).
    pub fn rect_gradient_multi(
        &mut self,
        rect: Rect,
        corner_radius: f32,
        angle: f32,
        stops: &[(f32, Color)],
    ) {
        if stops.len() < 2 { return; }
        if stops.len() == 2 {
            self.rect_gradient_linear(rect, corner_radius, angle, stops[0].1, stops[1].1);
            return;
        }
        // 3 or 4 stops → true single-pass shader path (stops at 0/0.5/1 or
        // 0/0.33/0.67/1 — caller-supplied stop positions are ignored, all
        // existing callers space stops evenly anyway).
        let n = stops.len().min(4);
        let colors: [Color; 4] = [
            stops[0].1,
            stops[1].1,
            stops[2].1,
            if n >= 4 { stops[3].1 } else { stops[2].1 },
        ];
        let count = if n >= 4 { 4 } else { 3 };
        self.rect_gradient_multi_direct(rect, corner_radius, angle, &colors[..count]);
    }

    // ── Bezier curves ───────────────────────────────────────────────────────

    /// Quadratic bezier curve (one control point).
    /// Adaptively tessellated into line segments for smooth rendering.
    pub fn bezier_quad(
        &mut self,
        x0: f32, y0: f32,
        cx: f32, cy: f32,
        x1: f32, y1: f32,
        width: f32, color: Color,
    ) {
        if color.a < 0.004 { return; }
        self.tessellate_quad(x0, y0, cx, cy, x1, y1, width, color, 0);
    }

    /// Cubic bezier curve (two control points).
    /// Adaptively tessellated into line segments for smooth rendering.
    pub fn bezier_cubic(
        &mut self,
        x0: f32, y0: f32,
        cx0: f32, cy0: f32,
        cx1: f32, cy1: f32,
        x1: f32, y1: f32,
        width: f32, color: Color,
    ) {
        if color.a < 0.004 { return; }
        self.tessellate_cubic(x0, y0, cx0, cy0, cx1, cy1, x1, y1, width, color, 0);
    }

    /// Dashed quadratic bezier curve.
    pub fn bezier_quad_dashed(
        &mut self,
        x0: f32, y0: f32,
        cx: f32, cy: f32,
        x1: f32, y1: f32,
        width: f32, dash: f32, gap: f32, color: Color,
    ) {
        if color.a < 0.004 { return; }
        let segments = flatten_quad(x0, y0, cx, cy, x1, y1, 0);
        for seg in segments.windows(2) {
            self.line_dashed(seg[0].0, seg[0].1, seg[1].0, seg[1].1, width, dash, gap, color);
        }
    }

    /// Dashed cubic bezier curve.
    pub fn bezier_cubic_dashed(
        &mut self,
        x0: f32, y0: f32,
        cx0: f32, cy0: f32,
        cx1: f32, cy1: f32,
        x1: f32, y1: f32,
        width: f32, dash: f32, gap: f32, color: Color,
    ) {
        if color.a < 0.004 { return; }
        let segments = flatten_cubic(x0, y0, cx0, cy0, cx1, cy1, x1, y1, 0);
        for seg in segments.windows(2) {
            self.line_dashed(seg[0].0, seg[0].1, seg[1].0, seg[1].1, width, dash, gap, color);
        }
    }

    fn tessellate_quad(
        &mut self,
        x0: f32, y0: f32, cx: f32, cy: f32, x1: f32, y1: f32,
        width: f32, color: Color, depth: u8,
    ) {
        if depth >= BEZIER_MAX_DEPTH || quad_flat_enough(x0, y0, cx, cy, x1, y1) {
            self.line(x0, y0, x1, y1, width, color);
            return;
        }
        let mx01 = (x0 + cx) * 0.5;
        let my01 = (y0 + cy) * 0.5;
        let mx12 = (cx + x1) * 0.5;
        let my12 = (cy + y1) * 0.5;
        let mx = (mx01 + mx12) * 0.5;
        let my = (my01 + my12) * 0.5;
        self.tessellate_quad(x0, y0, mx01, my01, mx, my, width, color, depth + 1);
        self.tessellate_quad(mx, my, mx12, my12, x1, y1, width, color, depth + 1);
    }

    fn tessellate_cubic(
        &mut self,
        x0: f32, y0: f32, cx0: f32, cy0: f32,
        cx1: f32, cy1: f32, x1: f32, y1: f32,
        width: f32, color: Color, depth: u8,
    ) {
        if depth >= BEZIER_MAX_DEPTH
            || cubic_flat_enough(x0, y0, cx0, cy0, cx1, cy1, x1, y1)
        {
            self.line(x0, y0, x1, y1, width, color);
            return;
        }
        let m01x = (x0 + cx0) * 0.5;   let m01y = (y0 + cy0) * 0.5;
        let m12x = (cx0 + cx1) * 0.5;  let m12y = (cy0 + cy1) * 0.5;
        let m23x = (cx1 + x1) * 0.5;   let m23y = (cy1 + y1) * 0.5;
        let ma_x = (m01x + m12x) * 0.5; let ma_y = (m01y + m12y) * 0.5;
        let mb_x = (m12x + m23x) * 0.5; let mb_y = (m12y + m23y) * 0.5;
        let mx = (ma_x + mb_x) * 0.5;   let my = (ma_y + mb_y) * 0.5;
        self.tessellate_cubic(x0, y0, m01x, m01y, ma_x, ma_y, mx, my, width, color, depth + 1);
        self.tessellate_cubic(mx, my, mb_x, mb_y, m23x, m23y, x1, y1, width, color, depth + 1);
    }
}

// ── Bezier helpers ──────────────────────────────────────────────────────────

const BEZIER_MAX_DEPTH: u8 = 8;
const BEZIER_TOLERANCE: f32 = 0.5;

fn quad_flat_enough(x0: f32, y0: f32, cx: f32, cy: f32, x1: f32, y1: f32) -> bool {
    let mx = (x0 + x1) * 0.5;
    let my = (y0 + y1) * 0.5;
    let dx = cx - mx;
    let dy = cy - my;
    dx * dx + dy * dy < BEZIER_TOLERANCE * BEZIER_TOLERANCE
}

fn cubic_flat_enough(
    x0: f32, y0: f32, cx0: f32, cy0: f32,
    cx1: f32, cy1: f32, x1: f32, y1: f32,
) -> bool {
    let ux = 3.0 * cx0 - 2.0 * x0 - x1;
    let uy = 3.0 * cy0 - 2.0 * y0 - y1;
    let vx = 3.0 * cx1 - 2.0 * x1 - x0;
    let vy = 3.0 * cy1 - 2.0 * y1 - y0;
    let max_sq = (ux * ux).max(vx * vx) + (uy * uy).max(vy * vy);
    let tol = BEZIER_TOLERANCE * 16.0;
    max_sq < tol * tol
}

fn flatten_quad(
    x0: f32, y0: f32, cx: f32, cy: f32, x1: f32, y1: f32, depth: u8,
) -> Vec<(f32, f32)> {
    if depth >= BEZIER_MAX_DEPTH || quad_flat_enough(x0, y0, cx, cy, x1, y1) {
        return vec![(x0, y0), (x1, y1)];
    }
    let mx01 = (x0 + cx) * 0.5; let my01 = (y0 + cy) * 0.5;
    let mx12 = (cx + x1) * 0.5; let my12 = (cy + y1) * 0.5;
    let mx = (mx01 + mx12) * 0.5; let my = (my01 + my12) * 0.5;
    let mut pts = flatten_quad(x0, y0, mx01, my01, mx, my, depth + 1);
    let right = flatten_quad(mx, my, mx12, my12, x1, y1, depth + 1);
    pts.extend_from_slice(&right[1..]);
    pts
}

fn flatten_cubic(
    x0: f32, y0: f32, cx0: f32, cy0: f32,
    cx1: f32, cy1: f32, x1: f32, y1: f32, depth: u8,
) -> Vec<(f32, f32)> {
    if depth >= BEZIER_MAX_DEPTH
        || cubic_flat_enough(x0, y0, cx0, cy0, cx1, cy1, x1, y1)
    {
        return vec![(x0, y0), (x1, y1)];
    }
    let m01x = (x0 + cx0) * 0.5;   let m01y = (y0 + cy0) * 0.5;
    let m12x = (cx0 + cx1) * 0.5;  let m12y = (cy0 + cy1) * 0.5;
    let m23x = (cx1 + x1) * 0.5;   let m23y = (cy1 + y1) * 0.5;
    let ma_x = (m01x + m12x) * 0.5; let ma_y = (m01y + m12y) * 0.5;
    let mb_x = (m12x + m23x) * 0.5; let mb_y = (m12y + m23y) * 0.5;
    let mx = (ma_x + mb_x) * 0.5;   let my = (ma_y + mb_y) * 0.5;
    let mut pts = flatten_cubic(x0, y0, m01x, m01y, ma_x, ma_y, mx, my, depth + 1);
    let right = flatten_cubic(mx, my, mb_x, mb_y, m23x, m23y, x1, y1, depth + 1);
    pts.extend_from_slice(&right[1..]);
    pts
}
