pub const SHADER_2D: &str = r#"
struct Globals {
    screen_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct InstanceInput {
    @location(0) bounds: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) params: vec4<f32>,
    @location(3) color_b: vec4<f32>,
    @location(4) color_c: vec4<f32>,
    @location(5) color_d: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) bounds: vec4<f32>,
    @location(3) params: vec4<f32>,
    @location(4) local_px: vec2<f32>,
    @location(5) color_b: vec4<f32>,
    @location(6) color_c: vec4<f32>,
    @location(7) color_d: vec4<f32>,
};

const SHAPE_RECT: f32 = 0.0;
const SHAPE_CIRCLE: f32 = 1.0;
const SHAPE_LINE: f32 = 2.0;
const SHAPE_RING: f32 = 3.0;
const SHAPE_GRADIENT_LINEAR: f32 = 4.0;
const SHAPE_GRADIENT_RADIAL: f32 = 5.0;
const SHAPE_RECT_STROKE: f32 = 6.0;
const SHAPE_RECT_4CORNER: f32 = 7.0;
const SHAPE_TRIANGLE: f32 = 8.0;
const SHAPE_SHADOW: f32 = 9.0;
const SHAPE_ARC: f32 = 10.0;
const SHAPE_DASHED_LINE: f32 = 11.0;
const SHAPE_INNER_SHADOW: f32 = 12.0;
const SHAPE_RECT_STROKE_PROGRESS: f32 = 13.0;
const SHAPE_TAPERED_PILL: f32 = 14.0;
const SHAPE_TAPERED_PILL_SHADOW: f32 = 15.0;
const SHAPE_TAPERED_PILL_INNER_SHADOW: f32 = 16.0;
const SHAPE_ROUNDED_RING: f32 = 17.0;
const SHAPE_GRADIENT_MULTI: f32 = 18.0;
const SHAPE_TWIN_RADIAL_GLOW: f32 = 22.0;
const SHAPE_RADIAL_GLOW: f32 = 23.0;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, instance: InstanceInput) -> VertexOutput {
    let uv = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));

    var quad_pos: vec2<f32>;
    var quad_size: vec2<f32>;

    if instance.params.w == SHAPE_LINE || instance.params.w == SHAPE_DASHED_LINE {
        let p1 = instance.bounds.xy;
        let p2 = instance.params.yz;
        let half_w = instance.params.x * 0.5 + 1.0;
        let min_pt = min(p1, p2) - vec2<f32>(half_w);
        let max_pt = max(p1, p2) + vec2<f32>(half_w);
        quad_pos = min_pt;
        quad_size = max_pt - min_pt;
    } else if instance.params.w == SHAPE_TRIANGLE {
        // Triangle: bounds.xy = p1, bounds.zw = p2, params.xy = p3
        let p1 = instance.bounds.xy;
        let p2 = instance.bounds.zw;
        let p3 = instance.params.xy;
        let min_pt = min(min(p1, p2), p3) - vec2<f32>(2.0);
        let max_pt = max(max(p1, p2), p3) + vec2<f32>(2.0);
        quad_pos = min_pt;
        quad_size = max_pt - min_pt;
    } else if instance.params.w == SHAPE_GRADIENT_RADIAL && instance.params.x == 0.0 {
        // Radial gradient with no corner radius: expand quad by 20% so the
        // gradient has room to fully fade before hitting the quad edge.
        // The fragment shader uses bounds (not quad) for the gradient math,
        // so this only adds extra pixels where t > 1.0 → fully edge-color.
        let expand = max(instance.bounds.z, instance.bounds.w) * 0.2;
        quad_pos = instance.bounds.xy - vec2<f32>(expand);
        quad_size = instance.bounds.zw + vec2<f32>(expand * 2.0);
    } else {
        quad_pos = instance.bounds.xy;
        quad_size = instance.bounds.zw;
    }

    let px = quad_pos + uv * quad_size;
    let ndc = vec2<f32>(
        px.x / globals.screen_size.x * 2.0 - 1.0,
        1.0 - px.y / globals.screen_size.y * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.color = instance.color;
    out.bounds = instance.bounds;
    out.params = instance.params;
    out.local_px = px;
    out.color_b = instance.color_b;
    out.color_c = instance.color_c;
    out.color_d = instance.color_d;
    return out;
}

// ── SDF helpers ─────────────────────────────────────────────────────────────

fn sdf_rounded_rect(p: vec2<f32>, center: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let d = abs(p - center) - half_size + vec2<f32>(radius);
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - radius;
}

// Per-corner radii: selects the correct radius based on which quadrant the pixel is in.
fn sdf_rounded_rect_4(p: vec2<f32>, center: vec2<f32>, half_size: vec2<f32>,
                       r_tl: f32, r_tr: f32, r_bl: f32, r_br: f32) -> f32 {
    let rel = p - center;
    var r: f32;
    if rel.x < 0.0 {
        if rel.y < 0.0 { r = r_tl; } else { r = r_bl; }
    } else {
        if rel.y < 0.0 { r = r_tr; } else { r = r_br; }
    }
    let d = abs(rel) - half_size + vec2<f32>(r);
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - r;
}

// Tapered pill: union of a full-height rounded rect on the left and a shorter
// rounded rect on the right. The tall rect's TR/BR corners use `taper_curve`
// (independent of the main `radius`) so the transition arc can be stretched
// out for a smoother step-down without affecting the rounded ends.
fn sdf_tapered_pill(p: vec2<f32>, bounds_min: vec2<f32>, bounds_max: vec2<f32>,
                    split_x_rel: f32, taper_amt: f32, radius: f32, taper_curve: f32) -> f32 {
    let size = bounds_max - bounds_min;
    let curve = max(taper_curve, radius);
    let tall_right = bounds_min.x + min(split_x_rel + curve, size.x);
    let tall_center = vec2<f32>(
        (bounds_min.x + tall_right) * 0.5,
        (bounds_min.y + bounds_max.y) * 0.5
    );
    let tall_half = vec2<f32>(max((tall_right - bounds_min.x) * 0.5, 0.0), size.y * 0.5);
    let r_end = min(radius, min(tall_half.x, tall_half.y));
    let r_curve = min(curve, min(tall_half.x, tall_half.y));
    let d_tall = sdf_rounded_rect_4(p, tall_center, tall_half, r_end, r_curve, r_end, r_curve);

    let short_left = bounds_min.x + split_x_rel;
    let short_center = vec2<f32>(
        (short_left + bounds_max.x) * 0.5,
        (bounds_min.y + bounds_max.y) * 0.5
    );
    let short_half = vec2<f32>(
        max((bounds_max.x - short_left) * 0.5, 0.0),
        max((size.y - taper_amt) * 0.5, 0.0)
    );
    let short_r = min(radius, min(short_half.x, short_half.y));
    let d_short = sdf_rounded_rect(p, short_center, short_half, short_r);

    return min(d_tall, d_short);
}

fn sdf_line(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

// Signed distance to a triangle (2D). Negative = inside.
fn sdf_triangle(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> f32 {
    let e0 = b - a; let e1 = c - b; let e2 = a - c;
    let v0 = p - a; let v1 = p - b; let v2 = p - c;

    let pq0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    let pq1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    let pq2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);

    let s = sign(e0.x * e2.y - e0.y * e2.x);
    let d0 = vec2<f32>(dot(pq0, pq0), s * (v0.x * e0.y - v0.y * e0.x));
    let d1 = vec2<f32>(dot(pq1, pq1), s * (v1.x * e1.y - v1.y * e1.x));
    let d2 = vec2<f32>(dot(pq2, pq2), s * (v2.x * e2.y - v2.y * e2.x));

    let d = min(min(d0, d1), d2);
    return -sqrt(d.x) * sign(d.y);
}

// Gaussian falloff for shadows. Full opacity inside the shape (dist < 0),
// smooth exponential decay outside.
fn shadow_mask(dist: f32, sigma: f32) -> f32 {
    if dist <= 0.0 { return 1.0; }
    let t = dist / (sigma + 0.001);
    return exp(-0.5 * t * t);
}

// Pseudo-random screen-space dither for gradients. Adds ±0.5/255 of noise
// per pixel so the quantization to 8-bit RGB happens to slightly different
// values for neighbouring pixels — visible banding on subtle gradients
// dissolves into film-grain. Per-channel noise (vs scalar) means the
// dithering carries chroma too instead of looking like monochrome static.
// Free in practice: ~5 ALU ops, no texture lookup.
fn dither_rgb(px: vec2<f32>) -> vec3<f32> {
    let n_r = fract(sin(dot(px, vec2<f32>(12.9898, 78.233))) * 43758.5453);
    let n_g = fract(sin(dot(px, vec2<f32>(63.7264, 10.873))) * 24634.6345);
    let n_b = fract(sin(dot(px, vec2<f32>(36.5829, 91.187))) * 51317.7345);
    return (vec3<f32>(n_r, n_g, n_b) - 0.5) * (1.0 / 255.0);
}

// ── Fragment shader ─────────────────────────────────────────────────────────

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var color = in.color;
    var mask: f32 = 1.0;

    if in.params.w == SHAPE_RECT {
        let radius = in.params.x;
        if radius < 0.5 {
            mask = 1.0;
        } else {
            let center = in.bounds.xy + in.bounds.zw * 0.5;
            let half_size = in.bounds.zw * 0.5;
            let dist = sdf_rounded_rect(in.local_px, center, half_size, radius);
            mask = 1.0 - smoothstep(-0.5, 0.5, dist);
        }

    } else if in.params.w == SHAPE_CIRCLE {
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let radius = min(in.bounds.z, in.bounds.w) * 0.5;
        let dist = length(in.local_px - center) - radius;
        mask = 1.0 - smoothstep(-1.0, 1.0, dist);

    } else if in.params.w == SHAPE_LINE {
        let dist = sdf_line(in.local_px, in.bounds.xy, in.params.yz) - in.params.x * 0.5;
        mask = 1.0 - smoothstep(-1.0, 1.0, dist);

    } else if in.params.w == SHAPE_RING {
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let radius = in.params.y;
        let dist = abs(length(in.local_px - center) - radius) - in.params.x * 0.5;
        mask = 1.0 - smoothstep(-1.0, 1.0, dist);

    } else if in.params.w == SHAPE_GRADIENT_LINEAR {
        let radius = in.params.x;
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let half_size = in.bounds.zw * 0.5;
        let dist = sdf_rounded_rect(in.local_px, center, half_size, radius);
        mask = 1.0 - smoothstep(-1.0, 1.0, dist);
        let angle = in.params.y;
        let dir = vec2<f32>(cos(angle), sin(angle));
        let rel = (in.local_px - in.bounds.xy) / in.bounds.zw - 0.5;
        // Project onto the gradient axis with full corner-to-corner coverage.
        // Without dividing by the corner-projection span the diagonal angles
        // (multiples of π/4) clip the gradient early — the same fix the
        // multi-stop path uses. Keeping the two paths aligned means switching
        // between 2- and 3-stop gradients doesn't visibly shift the band.
        let span = max(abs(dir.x) + abs(dir.y), 0.0001);
        let t = clamp((dot(rel, dir) / span) + 0.5, 0.0, 1.0);
        color = mix(in.color, in.color_b, vec4<f32>(t));
        color = vec4<f32>(color.rgb + dither_rgb(in.local_px), color.a);

    } else if in.params.w == SHAPE_GRADIENT_MULTI {
        // True multi-stop linear gradient (3 or 4 colors, evenly spaced).
        // params: [corner_radius, angle, stop_count (3.0 or 4.0), shape_id]
        // color = stop0, color_b = stop1, color_c = stop2, color_d = stop3.
        let radius = in.params.x;
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let half_size = in.bounds.zw * 0.5;
        let dist = sdf_rounded_rect(in.local_px, center, half_size, radius);
        mask = 1.0 - smoothstep(-1.0, 1.0, dist);
        let angle = in.params.y;
        let dir = vec2<f32>(cos(angle), sin(angle));
        let rel = (in.local_px - in.bounds.xy) / in.bounds.zw - 0.5;
        // Project onto the gradient axis and remap so the gradient runs from
        // one corner of the (axis-aligned) bounding rect to the opposite. The
        // total projection range for a unit square along `dir` is
        //   |cos(angle)| + |sin(angle)|
        // so dividing by that span gives a t∈[0,1] covering corner-to-corner
        // for any angle — including the diagonal 45° case that the simple
        // `dot + 0.5` form would otherwise clip.
        let span = max(abs(dir.x) + abs(dir.y), 0.0001);
        let t = clamp((dot(rel, dir) / span) + 0.5, 0.0, 1.0);
        let four_stops = in.params.z >= 3.5;
        if four_stops {
            // Stops at 0, 1/3, 2/3, 1
            if t < 0.33333333 {
                color = mix(in.color, in.color_b, vec4<f32>(t * 3.0));
            } else if t < 0.66666666 {
                color = mix(in.color_b, in.color_c, vec4<f32>((t - 0.33333333) * 3.0));
            } else {
                color = mix(in.color_c, in.color_d, vec4<f32>((t - 0.66666666) * 3.0));
            }
        } else {
            // 3 stops at 0, 0.5, 1
            if t < 0.5 {
                color = mix(in.color, in.color_b, vec4<f32>(t * 2.0));
            } else {
                color = mix(in.color_b, in.color_c, vec4<f32>((t - 0.5) * 2.0));
            }
        }
        color = vec4<f32>(color.rgb + dither_rgb(in.local_px), color.a);

    } else if in.params.w == SHAPE_GRADIENT_RADIAL {
        let radius = in.params.x;
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let half_size = in.bounds.zw * 0.5;
        if radius > 0.0 {
            // With corner radius, use SDF mask for the rounded shape
            let dist = sdf_rounded_rect(in.local_px, center, half_size, radius);
            mask = 1.0 - smoothstep(-1.0, 1.0, dist);
        } else {
            // No corner radius — skip SDF clipping entirely, let the gradient
            // colors handle alpha (avoids hard rectangular edges)
            mask = 1.0;
        }
        let max_dist = length(in.bounds.zw * 0.5);
        let d = length(in.local_px - center) / max_dist;
        let t = clamp(d, 0.0, 1.0);
        color = mix(in.color, in.color_b, vec4<f32>(t));
        color = vec4<f32>(color.rgb + dither_rgb(in.local_px), color.a);

    } else if in.params.w == SHAPE_TWIN_RADIAL_GLOW {
        // Two independent radial glows, each anchored at a normalized
        // position within the rect (color_c.xy and color_c.zw) and fading
        // smoothly to zero by `glow_radius_norm` * the inter-anchor
        // distance. With the default radius of 0.5 the two glows meet at
        // exactly the midpoint between them, both at zero alpha — so the
        // window's center stays its original bg color even with both
        // colors cranked to 100%. Anything between the anchors is a
        // weighted blend of the two colors.
        // params: [corner_radius, glow_radius_norm, _, shape_id]
        // color = color_a, color_b = color_b
        // color_c = [a_x, a_y, b_x, b_y] normalized 0-1 within rect
        let radius = in.params.x;
        let glow_r_norm = in.params.y;
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let half_size = in.bounds.zw * 0.5;
        let dist = sdf_rounded_rect(in.local_px, center, half_size, radius);
        mask = 1.0 - smoothstep(-1.0, 1.0, dist);

        let anchor_a = in.bounds.xy + in.color_c.xy * in.bounds.zw;
        let anchor_b = in.bounds.xy + in.color_c.zw * in.bounds.zw;
        let anchor_dist = max(length(anchor_b - anchor_a), 0.001);
        let glow_r = glow_r_norm * anchor_dist;

        let d_a = length(in.local_px - anchor_a);
        let d_b = length(in.local_px - anchor_b);
        let f_a = 1.0 - smoothstep(0.0, glow_r, d_a);
        let f_b = 1.0 - smoothstep(0.0, glow_r, d_b);

        let a_a = in.color.a * f_a;
        let a_b = in.color_b.a * f_b;
        let total_a = a_a + a_b;
        if total_a < 0.001 {
            color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        } else {
            let mixed_rgb = (in.color.rgb * a_a + in.color_b.rgb * a_b) / total_a;
            color = vec4<f32>(mixed_rgb, min(total_a, 1.0));
        }
        color = vec4<f32>(color.rgb + dither_rgb(in.local_px), color.a);

    } else if in.params.w == SHAPE_RADIAL_GLOW {
        // Single radial glow anchored at a normalized point within the
        // rect (color_b.xy), fading from `color`'s alpha at the anchor to
        // zero at `glow_radius_px` away. Stack several of these to build
        // the multi-corner window-background overlay.
        // params: [corner_radius, glow_radius_px, _, shape_id]
        // color_b.xy = anchor (normalized 0..1 within rect)
        let radius = in.params.x;
        let glow_r = max(in.params.y, 0.0001);
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let half_size = in.bounds.zw * 0.5;
        let dist = sdf_rounded_rect(in.local_px, center, half_size, radius);
        mask = 1.0 - smoothstep(-1.0, 1.0, dist);

        let anchor = in.bounds.xy + in.color_b.xy * in.bounds.zw;
        let d = length(in.local_px - anchor);
        let falloff = 1.0 - smoothstep(0.0, glow_r, d);
        color = vec4<f32>(in.color.rgb, in.color.a * falloff);
        color = vec4<f32>(color.rgb + dither_rgb(in.local_px), color.a);

    } else if in.params.w == SHAPE_RECT_STROKE {
        // Proper rounded rect outline via SDF
        // params: [corner_radius, stroke_width, 0, shape_id]
        let radius = in.params.x;
        let stroke_w = in.params.y;
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let half_size = in.bounds.zw * 0.5;
        let dist = sdf_rounded_rect(in.local_px, center, half_size, radius);
        // Hollow: abs(dist) gives distance to the boundary ring
        let ring_dist = abs(dist + stroke_w * 0.5) - stroke_w * 0.5;
        mask = 1.0 - smoothstep(-1.0, 1.0, ring_dist);

    } else if in.params.w == SHAPE_ROUNDED_RING {
        // CSS-style border: independent outer/inner SDFs so the inner corner
        // stays cleanly rounded for any thickness.
        // params: [outer_radius, thickness, 0, shape_id]
        let outer_r = in.params.x;
        let thickness = in.params.y;
        let inner_r = max(outer_r - thickness, 0.0);
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        // Un-expand the AA padding (CPU side expanded by 2.0)
        let outer_half = in.bounds.zw * 0.5 - vec2<f32>(2.0);
        let inner_half = max(outer_half - vec2<f32>(thickness), vec2<f32>(0.0));
        let outer_d = sdf_rounded_rect(in.local_px, center, outer_half, outer_r);
        let inner_d = sdf_rounded_rect(in.local_px, center, inner_half, inner_r);
        // Boolean difference: inside outer AND outside inner.
        let ring_d = max(outer_d, -inner_d);
        mask = 1.0 - smoothstep(-1.0, 1.0, ring_d);

    } else if in.params.w == SHAPE_RECT_STROKE_PROGRESS {
        // Rounded rect stroke with perimeter-based progress mask.
        // Sweeps counter-clockwise from top-left with uniform speed.
        // params: [corner_radius, stroke_width, progress(0-1), shape_id]
        let radius = in.params.x;
        let stroke_w = in.params.y;
        let progress = in.params.z;
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let half_size = in.bounds.zw * 0.5;

        // SDF stroke mask
        let dist = sdf_rounded_rect(in.local_px, center, half_size, radius);
        let ring_dist = abs(dist + stroke_w * 0.5) - stroke_w * 0.5;
        let stroke_mask = 1.0 - smoothstep(-1.0, 1.0, ring_dist);

        // Recover original rect half-size (undo CPU-side expansion)
        let hs = half_size - vec2<f32>(stroke_w * 0.5 + 2.0);

        // Inner rect corners (where straight edges meet corner arcs)
        let imin = center - hs + vec2<f32>(radius);
        let imax = center + hs - vec2<f32>(radius);

        // Segment lengths
        let ew = max(imax.x - imin.x, 0.001); // top/bottom straight edge
        let eh = max(imax.y - imin.y, 0.001); // left/right straight edge
        let aq = 1.5707963 * radius;           // quarter-circle arc length
        let total = 2.0 * ew + 2.0 * eh + 4.0 * aq;

        // Counter-clockwise from top-left:
        //   seg0: TL arc        [0,              aq]
        //   seg1: Left edge ↓   [aq,             aq + eh]
        //   seg2: BL arc        [aq + eh,        2aq + eh]
        //   seg3: Bottom edge → [2aq + eh,       2aq + eh + ew]
        //   seg4: BR arc        [2aq + eh + ew,  3aq + eh + ew]
        //   seg5: Right edge ↑  [3aq + eh + ew,  3aq + 2eh + ew]
        //   seg6: TR arc        [3aq + 2eh + ew, 4aq + 2eh + ew]
        //   seg7: Top edge ←    [4aq + 2eh + ew, total]

        let p = in.local_px;
        let hp = 1.5707963; // π/2
        var d: f32 = 0.0;

        if p.x < imin.x && p.y < imin.y {
            // TL corner arc
            let rel = p - vec2<f32>(imin.x, imin.y);
            let f = clamp(atan2(-rel.x, -rel.y) / hp, 0.0, 1.0);
            d = f * aq;
        } else if p.x < imin.x && p.y > imax.y {
            // BL corner arc
            let rel = p - vec2<f32>(imin.x, imax.y);
            let f = clamp(atan2(rel.y, -rel.x) / hp, 0.0, 1.0);
            d = aq + eh + f * aq;
        } else if p.x > imax.x && p.y > imax.y {
            // BR corner arc
            let rel = p - vec2<f32>(imax.x, imax.y);
            let f = clamp(atan2(rel.x, rel.y) / hp, 0.0, 1.0);
            d = 2.0 * aq + eh + ew + f * aq;
        } else if p.x > imax.x && p.y < imin.y {
            // TR corner arc
            let rel = p - vec2<f32>(imax.x, imin.y);
            let f = clamp(atan2(-rel.y, rel.x) / hp, 0.0, 1.0);
            d = 3.0 * aq + 2.0 * eh + ew + f * aq;
        } else if p.x < imin.x {
            // Left edge (going down)
            d = aq + clamp((p.y - imin.y) / eh, 0.0, 1.0) * eh;
        } else if p.y > imax.y {
            // Bottom edge (going right)
            d = 2.0 * aq + eh + clamp((p.x - imin.x) / ew, 0.0, 1.0) * ew;
        } else if p.x > imax.x {
            // Right edge (going up)
            d = 3.0 * aq + eh + ew + clamp((imax.y - p.y) / eh, 0.0, 1.0) * eh;
        } else if p.y < imin.y {
            // Top edge (going left)
            d = 4.0 * aq + 2.0 * eh + ew + clamp((imax.x - p.x) / ew, 0.0, 1.0) * ew;
        }

        let t = d / total;
        let feather = 3.0 / total;
        let progress_mask = 1.0 - smoothstep(progress - feather, progress + feather, t);

        mask = stroke_mask * progress_mask;

    } else if in.params.w == SHAPE_RECT_4CORNER {
        // Per-corner radii rect
        // params: [tl, tr, 0, shape_id], color_b: [bl, br, 0, 0]
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let half_size = in.bounds.zw * 0.5;
        let dist = sdf_rounded_rect_4(
            in.local_px, center, half_size,
            in.params.x, in.params.y, in.color_b.x, in.color_b.y
        );
        mask = 1.0 - smoothstep(-1.0, 1.0, dist);
        // color_b is used for radii, so use the main color
        color = in.color;

    } else if in.params.w == SHAPE_TRIANGLE {
        // bounds.xy = p1, bounds.zw = p2, params.xy = p3
        let dist = sdf_triangle(in.local_px, in.bounds.xy, in.bounds.zw, in.params.xy);
        mask = 1.0 - smoothstep(-1.0, 1.0, dist);

    } else if in.params.w == SHAPE_SHADOW {
        // Soft shadow for rounded rects
        // params: [corner_radius, blur_sigma, 0, shape_id]
        // bounds are expanded by 3*sigma — shrink back to get original rect SDF
        let radius = in.params.x;
        let sigma = in.params.y;
        let expand = 3.0 * sigma;
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let half_size = in.bounds.zw * 0.5 - vec2<f32>(expand);
        let dist = sdf_rounded_rect(in.local_px, center, half_size, radius);
        mask = shadow_mask(dist, sigma);

    } else if in.params.w == SHAPE_INNER_SHADOW {
        // Inset shadow for bevels. Shadow is drawn *inside* the rect.
        // params: [corner_radius, sigma, 0, shape_id]
        // color_b: [offset_x, offset_y, 0, 0]
        let radius = in.params.x;
        let sigma = in.params.y;
        let offset = in.color_b.xy;
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let half_size = in.bounds.zw * 0.5;
        // SDF of the container — clip to inside
        let container_dist = sdf_rounded_rect(in.local_px, center, half_size, radius);
        let inside = 1.0 - smoothstep(-0.5, 0.5, container_dist);
        // Offset SDF — shadow cast from the edges inward
        let offset_dist = sdf_rounded_rect(in.local_px - offset, center, half_size, radius);
        // Invert: we want shadow where we're close to (or outside) the offset boundary
        let shadow = shadow_mask(-offset_dist, sigma);
        mask = inside * shadow;

    } else if in.params.w == SHAPE_ARC {
        // Arc / pie slice
        // bounds.xy = center, bounds.zw = [outer_radius*2, outer_radius*2]
        // params: [stroke_width, start_angle, sweep_angle, shape_id]
        // If stroke_width == 0, draws a filled pie. Otherwise draws an arc stroke.
        let center = in.bounds.xy + in.bounds.zw * 0.5;
        let outer_r = min(in.bounds.z, in.bounds.w) * 0.5;
        let stroke_w = in.params.x;
        let start_a = in.params.y;
        let sweep = in.params.z;
        // color_b.x = inner_radius (for donut arcs), 0 = full pie
        let inner_r = in.color_b.x;

        let rel = in.local_px - center;
        let d = length(rel);
        let angle = atan2(rel.y, rel.x);

        // Normalize angle relative to start, wrap to [0, 2*PI)
        var a = angle - start_a;
        a = a - floor(a / 6.2831853) * 6.2831853;
        // Check if within sweep
        let in_sweep = a >= 0.0 && a <= sweep;

        if stroke_w > 0.0 {
            // Arc stroke mode
            let ring_dist = abs(d - (outer_r + inner_r) * 0.5) - stroke_w * 0.5;
            let radial_mask = 1.0 - smoothstep(-1.0, 1.0, ring_dist);
            if in_sweep {
                mask = radial_mask;
            } else {
                mask = 0.0;
            }
        } else {
            // Filled pie mode
            let outer_dist = d - outer_r;
            let outer_mask = 1.0 - smoothstep(-1.0, 1.0, outer_dist);
            let inner_mask = 1.0 - smoothstep(-1.0, 1.0, -(d - inner_r));
            if in_sweep {
                mask = outer_mask * inner_mask;
            } else {
                mask = 0.0;
            }
        }
        color = in.color;

    } else if in.params.w == SHAPE_DASHED_LINE {
        // Dashed line
        // bounds.xy = p1, params.yz = p2, params.x = width
        // color_b.x = dash_length, color_b.y = gap_length
        let p1 = in.bounds.xy;
        let p2 = in.params.yz;
        let dist = sdf_line(in.local_px, p1, p2) - in.params.x * 0.5;
        let line_mask = 1.0 - smoothstep(-1.0, 1.0, dist);

        // Project pixel onto line to get parametric position
        let line_dir = p2 - p1;
        let line_len = length(line_dir);
        let t = dot(in.local_px - p1, line_dir) / (line_len * line_len + 0.001);
        let along = t * line_len;
        let dash_len = in.color_b.x;
        let gap_len = in.color_b.y;
        let period = dash_len + gap_len;
        let phase = along - floor(along / period) * period;
        let dash_mask = 1.0 - smoothstep(dash_len - 1.0, dash_len, phase);

        mask = line_mask * dash_mask;
        color = in.color;

    } else if in.params.w == SHAPE_TAPERED_PILL {
        // Tapered pill fill
        // params: [radius, split_x_rel, taper_amt, shape_id]
        // color_b: [_, _, _, taper_curve]
        let radius = in.params.x;
        let split_x = in.params.y;
        let taper_amt = in.params.z;
        let taper_curve = in.color_b.w;
        let bounds_min = in.bounds.xy;
        let bounds_max = in.bounds.xy + in.bounds.zw;
        let dist = sdf_tapered_pill(in.local_px, bounds_min, bounds_max, split_x, taper_amt, radius, taper_curve);
        mask = 1.0 - smoothstep(-0.5, 0.5, dist);

    } else if in.params.w == SHAPE_TAPERED_PILL_SHADOW {
        // Soft drop shadow for a tapered pill
        // params: [radius, split_x_rel, taper_amt, shape_id]
        // color_b: [sigma, _, _, taper_curve]
        // bounds are expanded by 3*sigma — shrink back to get original rect SDF
        let radius = in.params.x;
        let split_x = in.params.y;
        let taper_amt = in.params.z;
        let sigma = in.color_b.x;
        let taper_curve = in.color_b.w;
        let expand = 3.0 * sigma;
        let bounds_min = in.bounds.xy + vec2<f32>(expand);
        let bounds_max = in.bounds.xy + in.bounds.zw - vec2<f32>(expand);
        let dist = sdf_tapered_pill(in.local_px, bounds_min, bounds_max, split_x, taper_amt, radius, taper_curve);
        mask = shadow_mask(dist, sigma);

    } else if in.params.w == SHAPE_TAPERED_PILL_INNER_SHADOW {
        // Inset shadow for a tapered pill — proper bevel that follows the taper curve.
        // params: [radius, split_x_rel, taper_amt, shape_id]
        // color_b: [sigma, offset_x, offset_y, taper_curve]
        let radius = in.params.x;
        let split_x = in.params.y;
        let taper_amt = in.params.z;
        let sigma = in.color_b.x;
        let offset = in.color_b.yz;
        let taper_curve = in.color_b.w;
        let bounds_min = in.bounds.xy;
        let bounds_max = in.bounds.xy + in.bounds.zw;
        let container_dist = sdf_tapered_pill(in.local_px, bounds_min, bounds_max, split_x, taper_amt, radius, taper_curve);
        let inside = 1.0 - smoothstep(-0.5, 0.5, container_dist);
        let offset_dist = sdf_tapered_pill(in.local_px - offset, bounds_min, bounds_max, split_x, taper_amt, radius, taper_curve);
        let shadow = shadow_mask(-offset_dist, sigma);
        mask = inside * shadow;
    }

    let alpha = color.a * mask;
    if alpha < 0.004 {
        discard;
    }

    return vec4<f32>(color.rgb * alpha, alpha);
}
"#;
