//! Text rendering: fontdue rasterizes glyphs, lantern_gpu's ported atlas +
//! pipeline pack and draw them. Implements the painter's `TextPass` so text
//! composites after the shape pass via `render_with_text`.

use std::collections::HashMap;

use lantern_gpu::glyph::{AtlasEntry, GlyphAtlas, GlyphPipeline, Quad};
use lantern_gpu::{Color, GpuContext, TextPass};

static FONT_BYTES: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

pub struct TextRenderer {
    font: fontdue::Font,
    atlas: GlyphAtlas,
    pipeline: GlyphPipeline,
    glyphs: HashMap<u64, (AtlasEntry, f32)>,
    /// Quads per render layer (0 = base, 1+ = overlays), matching the
    /// painter's layers so popups cover base text.
    layers: Vec<Vec<Quad>>,
    layer: usize,
}

/// Cache key: char + size quantized to tenths of a pixel.
fn key(ch: char, px: f32) -> u64 {
    ((ch as u64) << 24) | ((px * 10.0).round() as u64 & 0xFF_FFFF)
}

impl TextRenderer {
    pub fn new(gpu: &GpuContext) -> Self {
        let font = fontdue::Font::from_bytes(FONT_BYTES, fontdue::FontSettings::default())
            .expect("embedded DejaVuSans must parse");
        let atlas = GlyphAtlas::new(&gpu.device, 1024);
        let pipeline = GlyphPipeline::new(&gpu.device, gpu.format, &atlas);
        Self {
            font,
            atlas,
            pipeline,
            glyphs: HashMap::new(),
            layers: vec![Vec::new()],
            layer: 0,
        }
    }

    /// Queue subsequent text into `layer` (0 = base, 1+ = overlays).
    pub fn set_layer(&mut self, layer: u8) {
        self.layer = layer as usize;
        while self.layers.len() <= self.layer {
            self.layers.push(Vec::new());
        }
    }

    fn glyph(&mut self, gpu: &GpuContext, ch: char, px: f32) -> (AtlasEntry, f32) {
        let key = key(ch, px);
        if let Some(&cached) = self.glyphs.get(&key) {
            return cached;
        }
        let (metrics, coverage) = self.font.rasterize(ch, px);
        // Atlas `top` = pixels from the baseline up to the glyph's top edge;
        // fontdue's ymin is baseline→bottom in y-up coordinates.
        let entry = self.atlas.insert(
            &gpu.queue,
            key,
            metrics.width as u32,
            metrics.height as u32,
            metrics.xmin,
            metrics.ymin + metrics.height as i32,
            &coverage,
        );
        let cached = (entry, metrics.advance_width);
        self.glyphs.insert(key, cached);
        cached
    }

    /// Queue a run of text with its top-left corner at (x, y).
    /// Returns the advance width of the run.
    pub fn queue(
        &mut self,
        gpu: &GpuContext,
        text: &str,
        x: f32,
        y: f32,
        px: f32,
        color: Color,
    ) -> f32 {
        let ascent = self
            .font
            .horizontal_line_metrics(px)
            .map(|m| m.ascent)
            .unwrap_or(px * 0.8);
        let baseline = (y + ascent).round();
        let mut pen_x = x.round();
        let start_x = pen_x;
        for ch in text.chars() {
            let (entry, advance) = self.glyph(gpu, ch, px);
            if entry.width > 0 {
                self.layers[self.layer].push(Quad {
                    x: pen_x + entry.left as f32,
                    y: baseline - entry.top as f32,
                    w: entry.width as f32,
                    h: entry.height as f32,
                    uv_min: entry.uv_min,
                    uv_max: entry.uv_max,
                    color: [color.r, color.g, color.b, color.a],
                });
            }
            pen_x += advance;
        }
        pen_x - start_x
    }

    /// Width of `text` at `px` without queuing anything.
    pub fn measure(&mut self, text: &str, px: f32) -> f32 {
        text.chars()
            .map(|ch| self.font.metrics(ch, px).advance_width)
            .sum()
    }

    /// Queue text horizontally centered on `cx`, top edge at `y`.
    pub fn queue_centered(
        &mut self,
        gpu: &GpuContext,
        text: &str,
        cx: f32,
        y: f32,
        px: f32,
        color: Color,
    ) {
        let w = self.measure(text, px);
        self.queue(gpu, text, cx - w * 0.5, y, px, color);
    }

    /// Line height at `px`.
    pub fn line_height(&self, px: f32) -> f32 {
        self.font
            .horizontal_line_metrics(px)
            .map(|m| m.new_line_size)
            .unwrap_or(px * 1.2)
    }
}

impl TextPass for TextRenderer {
    fn render_text_layer(
        &mut self,
        gpu: &GpuContext,
        encoder: &mut lantern_gpu::wgpu::CommandEncoder,
        view: &lantern_gpu::wgpu::TextureView,
        layer: u8,
    ) {
        let Some(quads) = self.layers.get_mut(layer as usize) else {
            return;
        };
        if quads.is_empty() {
            return;
        }
        self.pipeline.render(
            &gpu.device,
            &gpu.queue,
            encoder,
            view,
            gpu.width(),
            gpu.height(),
            quads,
        );
        quads.clear();
    }
}
