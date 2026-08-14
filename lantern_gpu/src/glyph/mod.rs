//! Glyph rendering: a coverage atlas + textured-quad pipeline, ported from
//! Lantern DE's `lntrn-type`. Rasterization is the caller's job (lantern_ui
//! pairs this with fontdue); this module just packs coverage bitmaps and
//! draws positioned quads.

mod atlas;
mod pipeline;

pub use atlas::{AtlasEntry, GlyphAtlas};
pub use pipeline::{GlyphPipeline, Quad};
