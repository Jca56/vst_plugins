//! WgpuEditor — plugs lantern_gpu's painter into lantern_vst3's `Editor`
//! slot. A plugin describes its face as a draw closure over the painter;
//! this crate owns the GPU surface lifecycle on the editor's child window.

use std::ffi::c_void;
use std::ptr::null_mut;
use std::time::Instant;

use lantern_gpu::{Color, GpuContext, Painter};
use lantern_vst3::plugin::{Editor, ParamsHandle};

pub use lantern_gpu;

/// Per-frame info handed to the draw closure.
pub struct FrameInfo {
    pub width: f32,
    pub height: f32,
    /// Seconds since the editor was attached (for animation).
    pub time: f32,
    pub params: ParamsHandle,
}

pub struct WgpuEditor {
    width: u32,
    height: u32,
    clear: Color,
    params: ParamsHandle,
    draw: Box<dyn FnMut(&mut Painter, &FrameInfo)>,
    gpu: Option<(GpuContext, Painter)>,
    attached_at: Option<Instant>,
}

impl WgpuEditor {
    pub fn create(
        width: u32,
        height: u32,
        clear: Color,
        params: ParamsHandle,
        draw: impl FnMut(&mut Painter, &FrameInfo) + 'static,
    ) -> Box<dyn Editor> {
        Box::new(Self {
            width,
            height,
            clear,
            params,
            draw: Box::new(draw),
            gpu: None,
            attached_at: None,
        })
    }
}

impl Editor for WgpuEditor {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn attached(&mut self, hwnd: *mut c_void) {
        match GpuContext::from_hwnd(hwnd, null_mut(), self.width, self.height) {
            Ok(gpu) => {
                let painter = Painter::new(&gpu);
                self.gpu = Some((gpu, painter));
                self.attached_at = Some(Instant::now());
            }
            Err(err) => {
                // Leave gpu = None: render() becomes a no-op and the host
                // shows an empty (black) editor rather than crashing.
                eprintln!("[lantern-editor] GPU init failed: {err}");
            }
        }
    }

    fn removed(&mut self) {
        self.gpu = None;
        self.attached_at = None;
    }

    fn render(&mut self) {
        let Some((gpu, painter)) = self.gpu.as_mut() else {
            return;
        };
        painter.clear();
        let info = FrameInfo {
            width: gpu.width() as f32,
            height: gpu.height() as f32,
            time: self
                .attached_at
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0),
            params: self.params,
        };
        (self.draw)(painter, &info);
        if painter.render(gpu, self.clear).is_err() {
            // Surface lost/outdated: reconfigure and try again next frame.
            gpu.surface.configure(&gpu.device, &gpu.config);
        }
    }
}
