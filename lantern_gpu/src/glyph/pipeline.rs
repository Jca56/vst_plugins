//! wgpu render pipeline for glyph quads.
//!
//! Takes a list of positioned [`Quad`]s, expands each to two triangles, and
//! draws them sampling the coverage atlas. Premultiplied-alpha blending over the
//! existing target contents (`LoadOp::Load`) so text composites onto whatever
//! was drawn before it.



use super::atlas::GlyphAtlas;

/// A positioned, textured quad ready for the GPU. Pixel-space coordinates,
/// origin top-left.
#[derive(Clone, Copy, Debug)]
pub struct Quad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    /// Linear RGBA. Premultiply happens in the shader.
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ViewportUniform {
    size: [f32; 2],
    _pad: [f32; 2],
}

const INITIAL_QUADS: usize = 256;

pub struct GlyphPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertex_buffer: wgpu::Buffer,
    vertex_capacity: usize,
    verts: Vec<Vertex>,
}

impl GlyphPipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, atlas: &GlyphAtlas) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lntrn-type glyph shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lntrn-type viewport"),
            size: std::mem::size_of::<ViewportUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("lntrn-type bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                        count: None,
                    },
                ],
            });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lntrn-type bind group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(atlas.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(atlas.sampler()),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lntrn-type pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("lntrn-type pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 16,
                            shader_location: 2,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
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
                topology: wgpu::PrimitiveTopology::TriangleList,
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

        let vertex_capacity = INITIAL_QUADS * 6;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lntrn-type vertex buffer"),
            size: (vertex_capacity * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group,
            uniform,
            vertex_buffer,
            vertex_capacity,
            verts: Vec::with_capacity(vertex_capacity),
        }
    }

    /// Draw `quads` into `view`, sized to `width`×`height` pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        quads: &[Quad],
    ) {
        self.verts.clear();
        for q in quads {
            let (x0, y0, x1, y1) = (q.x, q.y, q.x + q.w, q.y + q.h);
            let [u0, v0] = q.uv_min;
            let [u1, v1] = q.uv_max;
            let c = q.color;
            // Two triangles: (TL, BL, BR) + (TL, BR, TR).
            self.verts.push(Vertex { pos: [x0, y0], uv: [u0, v0], color: c });
            self.verts.push(Vertex { pos: [x0, y1], uv: [u0, v1], color: c });
            self.verts.push(Vertex { pos: [x1, y1], uv: [u1, v1], color: c });
            self.verts.push(Vertex { pos: [x0, y0], uv: [u0, v0], color: c });
            self.verts.push(Vertex { pos: [x1, y1], uv: [u1, v1], color: c });
            self.verts.push(Vertex { pos: [x1, y0], uv: [u1, v0], color: c });
        }
        if self.verts.is_empty() {
            return;
        }

        if self.verts.len() > self.vertex_capacity {
            self.vertex_capacity = self.verts.len().next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("lntrn-type vertex buffer"),
                size: (self.vertex_capacity * std::mem::size_of::<Vertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        queue.write_buffer(&self.vertex_buffer, 0, crate::pod::cast_slice(&self.verts));
        let vp = ViewportUniform {
            size: [width as f32, height as f32],
            _pad: [0.0, 0.0],
        };
        queue.write_buffer(&self.uniform, 0, crate::pod::bytes_of(&vp));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("lntrn-type text pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.verts.len() as u32, 0..1);
    }
}

// Safety: repr(C); all fields are f32 arrays — no padding, no invalid bit patterns.
unsafe impl crate::pod::Pod for Vertex {}
unsafe impl crate::pod::Pod for ViewportUniform {}
