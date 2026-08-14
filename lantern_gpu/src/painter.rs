
use crate::gfx::{Frame, GpuContext};

use crate::color::Color;
use crate::rect::Rect;
use crate::shader::SHADER_2D;

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

#[repr(C)]
#[derive(Clone, Copy)]
struct Instance {
    bounds: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
    color_b: [f32; 4],
    color_c: [f32; 4],
    color_d: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Globals {
    screen_size: [f32; 2],
    _pad: [f32; 2],
}

/// Initial instance-buffer capacity. The buffer grows on demand in
/// `ensure_uploaded` — a full-screen TUI can legitimately need one shape
/// instance per grid cell (e.g. ~18k cells on a large window painting
/// per-cell backgrounds), so a fixed cap is not safe.
const INITIAL_INSTANCES: usize = 8192;

pub trait TextPass {
    fn render_text(
        &mut self,
        gpu: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    );
}

struct ClipSpan {
    start: u32,
    clip: Option<Rect>,
}

pub struct Painter {
    pipeline: wgpu::RenderPipeline,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    /// Current capacity (in instances) of `instance_buffer`.
    instance_capacity: usize,
    instances: Vec<Instance>,
    clip_stack: Vec<Rect>,
    clip_spans: Vec<ClipSpan>,
    /// Current layer being drawn into (0 = base, 1+ = overlay).
    current_layer: u8,
    /// Boundary markers: (instance_idx, clip_span_idx) where each layer ends.
    layer_breaks: Vec<(u32, usize)>,
    /// Whether instances have been uploaded to GPU this frame.
    instances_uploaded: bool,
}

impl Painter {
    pub fn new(gpu: &GpuContext) -> Self {
        let shader = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lantern 2D Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_2D.into()),
        });

        let globals_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let globals_layout = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Globals Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let globals_bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Globals Bind Group"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("2D Pipeline Layout"),
            bind_group_layouts: &[&globals_layout],
            immediate_size: 0,
        });

        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 32,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 48,
                    shader_location: 3,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 64,
                    shader_location: 4,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 80,
                    shader_location: 5,
                },
            ],
        };

        let pipeline = gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("2D Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[instance_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let instance_buffer = Self::create_instance_buffer(gpu, INITIAL_INSTANCES);

        Self {
            pipeline,
            globals_buffer,
            globals_bind_group,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCES,
            instances: Vec::with_capacity(1024),
            clip_stack: Vec::new(),
            clip_spans: vec![ClipSpan { start: 0, clip: None }],
            current_layer: 0,
            layer_breaks: Vec::new(),
            instances_uploaded: false,
        }
    }

    pub fn clear(&mut self) {
        self.instances.clear();
        self.clip_stack.clear();
        self.clip_spans.clear();
        self.clip_spans.push(ClipSpan { start: 0, clip: None });
        self.current_layer = 0;
        self.layer_breaks.clear();
        self.instances_uploaded = false;
    }

    /// Push a clip rectangle. Shapes drawn after this call will be clipped
    /// to the intersection of all active clip rects on the stack.
    pub fn push_clip(&mut self, rect: Rect) {
        let effective = if let Some(current) = self.clip_stack.last() {
            current.intersect(&rect).unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0))
        } else {
            rect
        };
        self.clip_stack.push(effective);
        self.clip_spans.push(ClipSpan {
            start: self.instances.len() as u32,
            clip: Some(effective),
        });
    }

    /// Pop the most recent clip rectangle.
    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
        let clip = self.clip_stack.last().copied();
        self.clip_spans.push(ClipSpan {
            start: self.instances.len() as u32,
            clip,
        });
    }

    /// Switch to a higher render layer. Layer 0 is base content, layer 1+
    /// is overlay content (menus, popups). Each layer gets its own painter
    /// and text sub-passes so overlays correctly cover underlying text.
    pub fn set_layer(&mut self, layer: u8) {
        if layer <= self.current_layer {
            return;
        }
        // Record where current layer ends
        self.layer_breaks
            .push((self.instances.len() as u32, self.clip_spans.len()));
        // Reset clip state for new layer
        self.clip_stack.clear();
        self.clip_spans
            .push(ClipSpan { start: self.instances.len() as u32, clip: None });
        self.current_layer = layer;
    }

    /// How many layers have content (at least 1).
    pub fn layer_count(&self) -> u8 {
        (self.layer_breaks.len() as u8) + 1
    }

    fn create_instance_buffer(gpu: &GpuContext, capacity: usize) -> wgpu::Buffer {
        gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Instance Buffer"),
            size: (capacity * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Upload instances to the GPU buffer. Called once before render_layer calls.
    fn ensure_uploaded(&mut self, gpu: &GpuContext) {
        if self.instances_uploaded {
            return;
        }
        if self.instances.len() > self.instance_capacity {
            let new_capacity = self.instances.len().next_power_of_two();
            self.instance_buffer = Self::create_instance_buffer(gpu, new_capacity);
            self.instance_capacity = new_capacity;
        }
        let globals = Globals {
            screen_size: [gpu.width() as f32, gpu.height() as f32],
            _pad: [0.0; 2],
        };
        gpu.queue
            .write_buffer(&self.globals_buffer, 0, crate::pod::bytes_of(&globals));
        if !self.instances.is_empty() {
            gpu.queue.write_buffer(
                &self.instance_buffer,
                0,
                crate::pod::cast_slice(&self.instances),
            );
        }
        self.instances_uploaded = true;
    }

    /// Render a specific layer's shapes. Layer 0 clears with `clear_color`,
    /// higher layers composite on top (LoadOp::Load).
    pub fn render_layer(
        &mut self,
        layer: u8,
        gpu: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        clear_color: Option<Color>,
    ) {
        let li = layer as usize;

        // Instance range for this layer
        let inst_start = if li == 0 {
            0u32
        } else if li <= self.layer_breaks.len() {
            self.layer_breaks[li - 1].0
        } else {
            return;
        };
        let inst_end = if li < self.layer_breaks.len() {
            self.layer_breaks[li].0
        } else {
            self.instances.len() as u32
        };

        // Clip-span range for this layer
        let span_start = if li == 0 {
            0usize
        } else {
            self.layer_breaks[li - 1].1
        };
        let span_end = if li < self.layer_breaks.len() {
            self.layer_breaks[li].1
        } else {
            self.clip_spans.len()
        };

        // Skip empty overlay layers (but always run layer 0 for the clear)
        if inst_start == inst_end && clear_color.is_none() {
            return;
        }

        self.ensure_uploaded(gpu);

        let load = match clear_color {
            Some(c) => wgpu::LoadOp::Clear(wgpu::Color {
                r: c.r as f64,
                g: c.g as f64,
                b: c.b as f64,
                a: c.a as f64,
            }),
            None => wgpu::LoadOp::Load,
        };

        let label = if layer == 0 {
            "Lantern Layer 0"
        } else {
            "Lantern Overlay Layer"
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });

        if inst_start < inst_end {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_vertex_buffer(0, self.instance_buffer.slice(..));

            for i in span_start..span_end {
                let span = &self.clip_spans[i];
                let end = if i + 1 < span_end {
                    self.clip_spans[i + 1].start
                } else {
                    inst_end
                };

                if end <= span.start {
                    continue;
                }

                match span.clip {
                    Some(rect) => {
                        let sx = (rect.x.max(0.0) as u32).min(gpu.width().saturating_sub(1));
                        let sy = (rect.y.max(0.0) as u32).min(gpu.height().saturating_sub(1));
                        let sw =
                            (rect.w.max(0.0) as u32).min(gpu.width().saturating_sub(sx));
                        let sh =
                            (rect.h.max(0.0) as u32).min(gpu.height().saturating_sub(sy));
                        if sw == 0 || sh == 0 {
                            continue;
                        }
                        pass.set_scissor_rect(sx, sy, sw, sh);
                    }
                    None => {
                        pass.set_scissor_rect(0, 0, gpu.width(), gpu.height());
                    }
                }

                pass.draw(0..4, span.start..end);
            }
        }
    }

    pub fn rect_filled(&mut self, rect: Rect, corner_radius: f32, color: Color) {
        if color.a < 0.004 { return; }
        self.instances.push(Instance {
            bounds: [rect.x, rect.y, rect.w, rect.h],
            color: [color.r, color.g, color.b, color.a],
            params: [corner_radius, 0.0, 0.0, SHAPE_RECT],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    pub fn circle_filled(&mut self, cx: f32, cy: f32, radius: f32, color: Color) {
        if color.a < 0.004 { return; }
        let size = radius * 2.0;
        self.instances.push(Instance {
            bounds: [cx - radius, cy - radius, size, size],
            color: [color.r, color.g, color.b, color.a],
            params: [0.0, 0.0, 0.0, SHAPE_CIRCLE],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: Color) {
        if color.a < 0.004 { return; }
        self.instances.push(Instance {
            bounds: [x1, y1, 0.0, 0.0],
            color: [color.r, color.g, color.b, color.a],
            params: [width, x2, y2, SHAPE_LINE],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    pub fn circle_stroke(&mut self, cx: f32, cy: f32, radius: f32, stroke_width: f32, color: Color) {
        if color.a < 0.004 { return; }
        let expand = stroke_width * 0.5 + 3.0;
        let size = (radius + expand) * 2.0;
        self.instances.push(Instance {
            bounds: [cx - radius - expand, cy - radius - expand, size, size],
            color: [color.r, color.g, color.b, color.a],
            params: [stroke_width, radius, 0.0, SHAPE_RING],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Draw a rounded rect with a linear gradient.
    /// `angle` is in radians: 0 = left→right, π/2 = top→bottom.
    pub fn rect_gradient_linear(
        &mut self,
        rect: Rect,
        corner_radius: f32,
        angle: f32,
        start: Color,
        end: Color,
    ) {
        self.instances.push(Instance {
            bounds: [rect.x, rect.y, rect.w, rect.h],
            color: [start.r, start.g, start.b, start.a],
            params: [corner_radius, angle, 0.0, SHAPE_GRADIENT_LINEAR],
            color_b: [end.r, end.g, end.b, end.a],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Draw a rounded rect with a true multi-stop linear gradient (3 or 4 colors).
    /// Renders in a single draw call — colors interpolate per-pixel in the shader.
    /// Stops are evenly spaced along the gradient axis (3 colors at 0, 0.5, 1.0;
    /// 4 colors at 0, 1/3, 2/3, 1.0). For 2 stops, prefer `rect_gradient_linear`.
    ///
    /// `angle` is in radians: 0 = left→right, π/2 = top→bottom.
    pub fn rect_gradient_multi_direct(
        &mut self,
        rect: Rect,
        corner_radius: f32,
        angle: f32,
        colors: &[Color],
    ) {
        if colors.len() < 3 { return; }
        let c0 = colors[0];
        let c1 = colors[1];
        let c2 = colors[2];
        let c3 = if colors.len() >= 4 { colors[3] } else { c2 };
        // params.z carries the stop count (3.0 or 4.0) — the shader derives
        // even spacing from it. Storing as float because params is vec4<f32>.
        let count = if colors.len() >= 4 { 4.0 } else { 3.0 };
        self.instances.push(Instance {
            bounds: [rect.x, rect.y, rect.w, rect.h],
            color: [c0.r, c0.g, c0.b, c0.a],
            params: [corner_radius, angle, count, SHAPE_GRADIENT_MULTI],
            color_b: [c1.r, c1.g, c1.b, c1.a],
            color_c: [c2.r, c2.g, c2.b, c2.a],
            color_d: [c3.r, c3.g, c3.b, c3.a],
        });
    }

    /// Single radial glow anchored at a normalized position within the rect.
    /// Color is at full alpha at the anchor and fades to zero at
    /// `glow_radius_px` away. Multiple calls stack — the window-background
    /// overlay uses one of these per active glow position (up to 4 corners
    /// + a center).
    pub fn rect_radial_glow(
        &mut self,
        rect: Rect,
        corner_radius: f32,
        anchor: (f32, f32),
        glow_radius_px: f32,
        color: Color,
    ) {
        self.instances.push(Instance {
            bounds: [rect.x, rect.y, rect.w, rect.h],
            color: [color.r, color.g, color.b, color.a],
            params: [corner_radius, glow_radius_px, 0.0, SHAPE_RADIAL_GLOW],
            color_b: [anchor.0, anchor.1, 0.0, 0.0],
            color_c: [0.0, 0.0, 0.0, 0.0],
            color_d: [0.0, 0.0, 0.0, 0.0],
        });
    }

    /// Twin radial glow — two independent radial fades, each anchored at a
    /// normalized position within the rect, fading to zero before they meet
    /// so the rect's interior between them keeps whatever color was drawn
    /// underneath. Used by the window-gradient overlay so the center of a
    /// window stays its base color even at 100% intensity.
    ///
    /// `anchor_a` / `anchor_b` are normalized (0..1) within the rect.
    /// `glow_radius_norm` is the falloff radius as a fraction of the
    /// distance between the two anchors — 0.5 means each glow fades to zero
    /// exactly at the midpoint between the anchors (no overlap, clean
    /// preservation of the center).
    pub fn rect_twin_radial_glow(
        &mut self,
        rect: Rect,
        corner_radius: f32,
        anchor_a: (f32, f32),
        anchor_b: (f32, f32),
        glow_radius_norm: f32,
        color_a: Color,
        color_b: Color,
    ) {
        self.instances.push(Instance {
            bounds: [rect.x, rect.y, rect.w, rect.h],
            color: [color_a.r, color_a.g, color_a.b, color_a.a],
            params: [corner_radius, glow_radius_norm, 0.0, SHAPE_TWIN_RADIAL_GLOW],
            color_b: [color_b.r, color_b.g, color_b.b, color_b.a],
            color_c: [anchor_a.0, anchor_a.1, anchor_b.0, anchor_b.1],
            color_d: [0.0, 0.0, 0.0, 0.0],
        });
    }

    /// Draw a rounded rect with a radial gradient (center → edge).
    pub fn rect_gradient_radial(
        &mut self,
        rect: Rect,
        corner_radius: f32,
        center_color: Color,
        edge_color: Color,
    ) {
        self.instances.push(Instance {
            bounds: [rect.x, rect.y, rect.w, rect.h],
            color: [center_color.r, center_color.g, center_color.b, center_color.a],
            params: [corner_radius, 0.0, 0.0, SHAPE_GRADIENT_RADIAL],
            color_b: [edge_color.r, edge_color.g, edge_color.b, edge_color.a],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Proper SDF-based rounded rect outline. Unlike `rect_stroke` which draws
    /// 4 separate rects, this produces smooth continuous corners.
    pub fn rect_stroke_sdf(&mut self, rect: Rect, corner_radius: f32, width: f32, color: Color) {
        if color.a < 0.004 { return; }
        let expand = width * 0.5 + 2.0;
        let expanded = rect.expand(expand);
        self.instances.push(Instance {
            bounds: [expanded.x, expanded.y, expanded.w, expanded.h],
            color: [color.r, color.g, color.b, color.a],
            params: [corner_radius, width, 0.0, SHAPE_RECT_STROKE],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// CSS-style rounded-rect border. Outer corner is `outer_radius`, inner
    /// corner is `outer_radius - width` (clamped ≥ 0), so thickness stays
    /// constant along straight edges AND the inner corner stays cleanly
    /// rounded for any thickness — unlike `rect_stroke_sdf` which collapses
    /// to a square inner corner once width approaches the outer radius.
    pub fn rect_border(&mut self, rect: Rect, outer_radius: f32, width: f32, color: Color) {
        if color.a < 0.004 || width <= 0.0 { return; }
        let expanded = rect.expand(2.0);
        self.instances.push(Instance {
            bounds: [expanded.x, expanded.y, expanded.w, expanded.h],
            color: [color.r, color.g, color.b, color.a],
            params: [outer_radius, width, 0.0, SHAPE_ROUNDED_RING],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// SDF rounded-rect stroke masked by a clockwise progress value.
    ///
    /// `progress` ranges from 1.0 (full border) to 0.0 (invisible),
    /// sweeping clockwise from the top-left corner.
    pub fn rect_stroke_progress(
        &mut self, rect: Rect, corner_radius: f32, width: f32, color: Color, progress: f32,
    ) {
        if color.a < 0.004 || progress <= 0.0 { return; }
        let expand = width * 0.5 + 2.0;
        let expanded = rect.expand(expand);
        self.instances.push(Instance {
            bounds: [expanded.x, expanded.y, expanded.w, expanded.h],
            color: [color.r, color.g, color.b, color.a],
            params: [corner_radius, width, progress.clamp(0.0, 1.0), SHAPE_RECT_STROKE_PROGRESS],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Rounded rect with different radius per corner.
    /// Order: top-left, top-right, bottom-left, bottom-right.
    pub fn rect_4corner(&mut self, rect: Rect, radii: [f32; 4], color: Color) {
        if color.a < 0.004 { return; }
        self.instances.push(Instance {
            bounds: [rect.x, rect.y, rect.w, rect.h],
            color: [color.r, color.g, color.b, color.a],
            params: [radii[0], radii[1], 0.0, SHAPE_RECT_4CORNER],
            color_b: [radii[2], radii[3], 0.0, 0.0],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Filled triangle from 3 points.
    pub fn triangle(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32, color: Color) {
        if color.a < 0.004 { return; }
        self.instances.push(Instance {
            bounds: [x1, y1, x2, y2],
            color: [color.r, color.g, color.b, color.a],
            params: [x3, y3, 0.0, SHAPE_TRIANGLE],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Soft drop shadow for a rounded rect. `sigma` controls blur spread.
    /// `offset_x` / `offset_y` shift the shadow (positive = right/down).
    pub fn shadow(
        &mut self, rect: Rect, corner_radius: f32, sigma: f32, color: Color,
        offset_x: f32, offset_y: f32,
    ) {
        if color.a < 0.004 { return; }
        let expand = sigma * 3.0;
        let shifted = Rect::new(rect.x + offset_x, rect.y + offset_y, rect.w, rect.h);
        let expanded = shifted.expand(expand);
        self.instances.push(Instance {
            bounds: [expanded.x, expanded.y, expanded.w, expanded.h],
            color: [color.r, color.g, color.b, color.a],
            params: [corner_radius, sigma, 0.0, SHAPE_SHADOW],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Inset (inner) shadow for bevels and pressed effects.
    /// The shadow is drawn *inside* the rect. `offset_x` / `offset_y` control
    /// the light direction (e.g. negative Y = light from top).
    pub fn inner_shadow(
        &mut self, rect: Rect, corner_radius: f32, sigma: f32, color: Color,
        offset_x: f32, offset_y: f32,
    ) {
        if color.a < 0.004 { return; }
        self.instances.push(Instance {
            bounds: [rect.x, rect.y, rect.w, rect.h],
            color: [color.r, color.g, color.b, color.a],
            params: [corner_radius, sigma, 0.0, SHAPE_INNER_SHADOW],
            color_b: [offset_x, offset_y, 0.0, 0.0],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Filled tapered pill — a single shape whose right portion is `taper_amt`
    /// shorter than the left, joined by a smooth step-down at `split_x`
    /// (measured from `rect.x`). `corner_radius` rounds both ends; the
    /// transition arc uses `taper_curve` so the curve length is independent
    /// of the end radii (pass ≥ `corner_radius`).
    pub fn tapered_pill(
        &mut self, rect: Rect, corner_radius: f32, split_x: f32, taper_amt: f32,
        taper_curve: f32, color: Color,
    ) {
        if color.a < 0.004 { return; }
        self.instances.push(Instance {
            bounds: [rect.x, rect.y, rect.w, rect.h],
            color: [color.r, color.g, color.b, color.a],
            params: [corner_radius, split_x, taper_amt, SHAPE_TAPERED_PILL],
            color_b: [0.0, 0.0, 0.0, taper_curve],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Soft drop shadow for a tapered pill.
    pub fn tapered_pill_shadow(
        &mut self, rect: Rect, corner_radius: f32, split_x: f32, taper_amt: f32,
        taper_curve: f32, sigma: f32, color: Color, offset_x: f32, offset_y: f32,
    ) {
        if color.a < 0.004 { return; }
        let expand = sigma * 3.0;
        let shifted = Rect::new(rect.x + offset_x, rect.y + offset_y, rect.w, rect.h);
        let expanded = shifted.expand(expand);
        self.instances.push(Instance {
            bounds: [expanded.x, expanded.y, expanded.w, expanded.h],
            color: [color.r, color.g, color.b, color.a],
            params: [corner_radius, split_x, taper_amt, SHAPE_TAPERED_PILL_SHADOW],
            color_b: [sigma, 0.0, 0.0, taper_curve],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Inset shadow for a tapered pill — bevel that follows the taper curve.
    pub fn tapered_pill_inner_shadow(
        &mut self, rect: Rect, corner_radius: f32, split_x: f32, taper_amt: f32,
        taper_curve: f32, sigma: f32, color: Color, offset_x: f32, offset_y: f32,
    ) {
        if color.a < 0.004 { return; }
        self.instances.push(Instance {
            bounds: [rect.x, rect.y, rect.w, rect.h],
            color: [color.r, color.g, color.b, color.a],
            params: [corner_radius, split_x, taper_amt, SHAPE_TAPERED_PILL_INNER_SHADOW],
            color_b: [sigma, offset_x, offset_y, taper_curve],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Arc stroke or filled pie slice.
    /// `start_angle` and `sweep_angle` in radians (0 = right, π/2 = down).
    /// If `stroke_width` > 0, draws an arc stroke. Otherwise draws a filled pie.
    /// `inner_radius` creates a donut shape (0 for full pie).
    pub fn arc(
        &mut self,
        cx: f32, cy: f32, outer_radius: f32,
        start_angle: f32, sweep_angle: f32,
        stroke_width: f32, inner_radius: f32,
        color: Color,
    ) {
        if color.a < 0.004 { return; }
        let expand = if stroke_width > 0.0 { stroke_width * 0.5 + 2.0 } else { 2.0 };
        let size = (outer_radius + expand) * 2.0;
        self.instances.push(Instance {
            bounds: [cx - outer_radius - expand, cy - outer_radius - expand, size, size],
            color: [color.r, color.g, color.b, color.a],
            params: [stroke_width, start_angle, sweep_angle, SHAPE_ARC],
            color_b: [inner_radius, 0.0, 0.0, 0.0],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Dashed line between two points.
    pub fn line_dashed(
        &mut self,
        x1: f32, y1: f32, x2: f32, y2: f32,
        width: f32, dash: f32, gap: f32,
        color: Color,
    ) {
        if color.a < 0.004 { return; }
        self.instances.push(Instance {
            bounds: [x1, y1, 0.0, 0.0],
            color: [color.r, color.g, color.b, color.a],
            params: [width, x2, y2, SHAPE_DASHED_LINE],
            color_b: [dash, gap, 0.0, 0.0],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
        });
    }

    /// Legacy rounded rect stroke using 4 filled rects. Prefer `rect_stroke_sdf` for
    /// proper rounded corners.
    pub fn rect_stroke(&mut self, rect: Rect, corner_radius: f32, width: f32, color: Color) {
        self.rect_filled(Rect::new(rect.x, rect.y, rect.w, width), corner_radius.min(width), color);
        self.rect_filled(Rect::new(rect.x, rect.y + rect.h - width, rect.w, width), corner_radius.min(width), color);
        self.rect_filled(Rect::new(rect.x, rect.y + width, width, rect.h - width * 2.0), 0.0, color);
        self.rect_filled(Rect::new(rect.x + rect.w - width, rect.y + width, width, rect.h - width * 2.0), 0.0, color);
    }

    pub fn render_pass(
        &mut self,
        gpu: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        clear_color: Color,
    ) {
        let load = wgpu::LoadOp::Clear(wgpu::Color {
            r: clear_color.r as f64, g: clear_color.g as f64,
            b: clear_color.b as f64, a: clear_color.a as f64,
        });
        self.execute_pass(gpu, encoder, view, load, "Lantern 2D Pass");
    }

    /// Like render_pass, but uses LoadOp::Load instead of Clear.
    /// Use for compositing shapes on top of existing framebuffer content.
    pub fn render_pass_overlay(
        &mut self,
        gpu: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) {
        self.execute_pass(gpu, encoder, view, wgpu::LoadOp::Load, "Lantern 2D Overlay");
    }

    fn execute_pass(
        &mut self,
        gpu: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        load: wgpu::LoadOp<wgpu::Color>,
        label: &str,
    ) {
        self.ensure_uploaded(gpu);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });

        if !self.instances.is_empty() {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_vertex_buffer(0, self.instance_buffer.slice(..));

            let total = self.instances.len() as u32;
            let span_count = self.clip_spans.len();

            for i in 0..span_count {
                let span = &self.clip_spans[i];
                let end = if i + 1 < span_count {
                    self.clip_spans[i + 1].start
                } else {
                    total
                };

                if end <= span.start { continue; }

                match span.clip {
                    Some(rect) => {
                        let sx = (rect.x.max(0.0) as u32).min(gpu.width().saturating_sub(1));
                        let sy = (rect.y.max(0.0) as u32).min(gpu.height().saturating_sub(1));
                        let sw = (rect.w.max(0.0) as u32).min(gpu.width().saturating_sub(sx));
                        let sh = (rect.h.max(0.0) as u32).min(gpu.height().saturating_sub(sy));
                        if sw == 0 || sh == 0 { continue; }
                        pass.set_scissor_rect(sx, sy, sw, sh);
                    }
                    None => {
                        pass.set_scissor_rect(0, 0, gpu.width(), gpu.height());
                    }
                }

                pass.draw(0..4, span.start..end);
            }
        }
    }

    pub fn render(&mut self, gpu: &GpuContext, clear_color: Color) -> Result<(), wgpu::SurfaceError> {
        let mut frame = gpu.begin_frame("Lantern 2D Encoder")?;
        self.render_into(gpu, &mut frame, clear_color);
        frame.submit(&gpu.queue);
        Ok(())
    }

    pub fn render_into(&mut self, gpu: &GpuContext, frame: &mut Frame, clear_color: Color) {
        let view = frame.view().clone();
        self.render_pass(gpu, frame.encoder_mut(), &view, clear_color);
    }

    pub fn render_with_text(
        &mut self,
        gpu: &GpuContext,
        text: &mut impl TextPass,
        clear_color: Color,
    ) -> Result<(), wgpu::SurfaceError> {
        let mut frame = gpu.begin_frame("Lantern 2D+Text Encoder")?;
        self.render_into(gpu, &mut frame, clear_color);
        let view = frame.view().clone();
        text.render_text(gpu, frame.encoder_mut(), &view);
        frame.submit(&gpu.queue);
        Ok(())
    }
}



// Safety: repr(C); all fields are f32 arrays — no padding, no invalid bit patterns.
unsafe impl crate::pod::Pod for Instance {}
unsafe impl crate::pod::Pod for Globals {}
