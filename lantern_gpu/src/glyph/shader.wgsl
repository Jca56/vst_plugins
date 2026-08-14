// Lantern text engine — glyph quad shader (Phase 0).
//
// Draws textured quads sampling a single-channel (R8) coverage atlas. Color is
// passed per-vertex in LINEAR space; the sRGB render target hardware-encodes on
// write, so we output premultiplied-alpha linear color and blend with
// (One, OneMinusSrcAlpha).

struct Viewport {
    size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
) -> VsOut {
    // Pixel coordinates (origin top-left, y down) → NDC.
    let ndc = vec2<f32>(
        pos.x / viewport.size.x * 2.0 - 1.0,
        1.0 - pos.y / viewport.size.y * 2.0,
    );
    var out: VsOut;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas_tex, atlas_samp, in.uv).r;
    let a = in.color.a * coverage;
    // Premultiplied output.
    return vec4<f32>(in.color.rgb * a, a);
}
