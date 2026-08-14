//! Face preview: render a plugin face offscreen and write it to a PNG —
//! iterate on layout and color in seconds instead of Ableton relaunches.
//!
//! Runs the face closure for a number of warm-up frames so meter smoothing
//! and display holds settle, reads the final frame back from the GPU, and
//! encodes it as an uncompressed-deflate PNG (ours, no deps).

use lantern_gpu::wgpu;
use lantern_gpu::{GpuContext, Painter, TextPass};
use lantern_vst3::plugin::ParamsHandle;

use crate::text::TextRenderer;
use crate::theme::Theme;
use crate::ui::{InputState, Ui, UiMemory};

/// Render `build` at (width, height) for `frames` frames and write the
/// final frame to `path`.
pub fn render_png(
    width: u32,
    height: u32,
    theme: &Theme,
    params: ParamsHandle,
    frames: u32,
    path: &str,
    mut build: impl FnMut(&mut Ui),
) -> Result<(), String> {
    let gpu = GpuContext::offscreen(width, height).map_err(|e| e.to_string())?;
    let mut painter = Painter::new(&gpu);
    let mut text = TextRenderer::new(&gpu);
    let mut input = InputState::default();
    let mut mem = UiMemory::default();

    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("face-preview"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: gpu.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    for frame in 0..frames.max(1) {
        painter.clear();
        let mut ui = Ui {
            painter: &mut painter,
            text: &mut text,
            gpu: &gpu,
            theme,
            width: width as f32,
            height: height as f32,
            time: frame as f32 / 60.0,
            params,
            input: &mut input,
            mem: &mut mem,
            size_request: None,
        };
        build(&mut ui);
        input.end_frame();

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("face-preview"),
            });
        for layer in 0..painter.layer_count() {
            let clear = if layer == 0 { Some(theme.bg) } else { None };
            painter.render_layer(layer, &gpu, &mut encoder, &view, clear);
            text.render_text_layer(&gpu, &mut encoder, &view, layer);
        }
        gpu.queue.submit(Some(encoder.finish()));
    }

    // Read the final frame back (rows padded to 256 bytes for the copy).
    let bytes_per_row = (width * 4 + 255) & !255;
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("face-preview-readback"),
        size: bytes_per_row as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("face-preview-copy"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit(Some(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = gpu.device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("readback map failed: {e:?}"))?;

    let data = slice.get_mapped_range();
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * bytes_per_row) as usize;
        let row_bytes = &data[start..start + (width * 4) as usize];
        for px in row_bytes.chunks_exact(4) {
            // Force opaque: viewers shouldn't composite the panel shadow.
            rgba.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
        }
    }
    drop(data);

    std::fs::write(path, png_encode(width, height, &rgba)).map_err(|e| e.to_string())
}

// ============================================================================
// Minimal PNG writer: 8-bit RGBA, zlib stream of stored (uncompressed)
// deflate blocks. Big files, zero dependencies, universally readable.
// ============================================================================

fn png_encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    // Scanlines, each prefixed with filter byte 0 (None).
    let stride = (width * 4) as usize;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in 0..height as usize {
        raw.push(0);
        raw.extend_from_slice(&rgba[row * stride..(row + 1) * stride]);
    }

    // zlib: header, stored deflate blocks, adler32.
    let mut z = vec![0x78, 0x01];
    let n_blocks = raw.len().div_ceil(65_535).max(1);
    for (i, block) in raw.chunks(65_535).enumerate() {
        z.push(u8::from(i + 1 == n_blocks));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = Vec::with_capacity(z.len() + 64);
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace
    write_chunk(&mut png, b"IHDR", &ihdr);
    write_chunk(&mut png, b"IDAT", &z);
    write_chunk(&mut png, b"IEND", &[]);
    png
}

fn write_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut crc = Crc32::new();
    crc.update(tag);
    crc.update(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

struct Crc32 {
    state: u32,
}

impl Crc32 {
    fn new() -> Self {
        Self { state: 0xFFFF_FFFF }
    }

    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.state ^= byte as u32;
            for _ in 0..8 {
                let mask = (self.state & 1).wrapping_neg();
                self.state = (self.state >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
    }

    fn finish(self) -> u32 {
        !self.state
    }
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5_552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}
