//! Lantern 2D GPU renderer.
//!
//! Ported from Lantern DE's `lntrn-render` (`gfx` device layer + `draw` SDF
//! painter), retargeted at Win32 windows so plugin GUIs can render inside
//! Ableton-under-Wine. The painter files (`color`, `rect`, `shader`,
//! `shapes`, `painter`) are near-verbatim copies — keep them in sync with
//! Lantern DE by hand when either side improves.

pub mod gfx;
pub mod glyph;
pub mod image;
mod pod;

mod color;
mod painter;
mod rect;
mod shader;
mod shapes;

pub use color::Color;
pub use gfx::{Frame, FrameTimingSnapshot, GpuContext};
pub use image::{BackgroundImage, ImagePass};
pub use painter::{Painter, TextPass};
pub use rect::Rect;

// Re-export wgpu so downstream crates don't need their own wgpu dependency
// (and can never drift to a different wgpu version than the renderer).
pub use wgpu;
