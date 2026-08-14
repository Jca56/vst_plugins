//! Glyph coverage atlas.
//!
//! A single R8Unorm texture into which rasterized glyph coverage bitmaps are
//! packed with a simple shelf packer. Each entry records its UV rectangle plus
//! the glyph's pixel size and bearing for placement at queue time.
//!
//! Phase 0 scope: insert + lookup only. Growth, multi-page atlases, and
//! per-glyph LRU eviction arrive in Phase 4 (`hint`/quality) and Phase 3
//! (caching) respectively. If the single page fills, further glyphs are dropped
//! (logged) rather than silently corrupting the layout.

use std::collections::HashMap;

/// A packed glyph's location in the atlas + its placement metrics.
// `left`/`top` bearings are consumed by glyph placement in Phase 1; `get` by the
// glyph cache in Phase 3. Kept now so the atlas API is stable across phases.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AtlasEntry {
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub width: u32,
    pub height: u32,
    /// Horizontal bearing: pixels from the pen origin to the glyph's left edge.
    pub left: i32,
    /// Vertical bearing: pixels from the baseline up to the glyph's top edge.
    pub top: i32,
}

pub struct GlyphAtlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    size: u32,
    pad: u32,
    cursor_x: u32,
    cursor_y: u32,
    shelf_h: u32,
    entries: HashMap<u64, AtlasEntry>,
}

impl GlyphAtlas {
    pub fn new(device: &wgpu::Device, size: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lntrn-type glyph atlas"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // Nearest sampling: glyphs are placed 1:1 texel→pixel, so the
        // anti-aliasing lives in the coverage values, not in filtering. Pairs
        // with a NonFiltering sampler binding (no `filterable` feature needed).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lntrn-type atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        Self {
            texture,
            view,
            sampler,
            size,
            pad: 1,
            cursor_x: 1,
            cursor_y: 1,
            shelf_h: 0,
            entries: HashMap::new(),
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    #[allow(dead_code)]
    pub fn get(&self, key: u64) -> Option<AtlasEntry> {
        self.entries.get(&key).copied()
    }

    /// Insert a coverage bitmap (`width * height` bytes, R8, row-major) under
    /// `key`. `left`/`top` are the glyph bearings. Returns the resulting entry
    /// (also cached). Re-inserting a present key returns the cached entry.
    pub fn insert(
        &mut self,
        queue: &wgpu::Queue,
        key: u64,
        width: u32,
        height: u32,
        left: i32,
        top: i32,
        coverage: &[u8],
    ) -> AtlasEntry {
        if let Some(existing) = self.entries.get(&key) {
            return *existing;
        }

        // Whitespace / empty glyph: record a zero-area entry (still advances).
        if width == 0 || height == 0 || coverage.is_empty() {
            let entry = AtlasEntry {
                width,
                height,
                left,
                top,
                ..AtlasEntry::default()
            };
            self.entries.insert(key, entry);
            return entry;
        }

        // Shelf packing: wrap to a new shelf when the row is full.
        if self.cursor_x + width + self.pad > self.size {
            self.cursor_x = self.pad;
            self.cursor_y += self.shelf_h + self.pad;
            self.shelf_h = 0;
        }
        if self.cursor_y + height > self.size {
            eprintln!(
                "[lntrn-type] glyph atlas full ({0}x{0}); dropping glyph key={1} \
                 — atlas growth lands in Phase 4",
                self.size, key
            );
            let entry = AtlasEntry {
                left,
                top,
                ..AtlasEntry::default()
            };
            self.entries.insert(key, entry);
            return entry;
        }

        let x = self.cursor_x;
        let y = self.cursor_y;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            coverage,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let s = self.size as f32;
        let entry = AtlasEntry {
            uv_min: [x as f32 / s, y as f32 / s],
            uv_max: [(x + width) as f32 / s, (y + height) as f32 / s],
            width,
            height,
            left,
            top,
        };

        self.cursor_x += width + self.pad;
        self.shelf_h = self.shelf_h.max(height);
        self.entries.insert(key, entry);
        entry
    }
}
