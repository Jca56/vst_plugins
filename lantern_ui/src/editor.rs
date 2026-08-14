//! UiEditor: the editor shell. Owns the GPU surface, painter, text renderer,
//! and widget state; forwards host mouse events into `InputState`; runs the
//! plugin's build closure once per frame.

use std::ffi::c_void;
use std::ptr::null_mut;
use std::time::Instant;

use lantern_gpu::{GpuContext, Painter};
use lantern_vst3::plugin::{Editor, MouseInput, ParamsHandle};

use crate::text::TextRenderer;
use crate::theme::Theme;
use crate::ui::{InputState, Ui, UiMemory};

struct GpuState {
    gpu: GpuContext,
    painter: Painter,
    text: TextRenderer,
}

pub struct UiEditor {
    width: u32,
    height: u32,
    theme: Theme,
    params: ParamsHandle,
    build: Box<dyn FnMut(&mut Ui)>,
    state: Option<GpuState>,
    input: InputState,
    mem: UiMemory,
    attached_at: Option<Instant>,
    /// Last char synthesized from a raw keydown (WM_CHAR echo suppression).
    last_synth: Option<char>,
    /// Size the face asked for, awaiting host negotiation.
    pending_resize: Option<(u32, u32)>,
}

impl UiEditor {
    pub fn create(
        width: u32,
        height: u32,
        theme: Theme,
        params: ParamsHandle,
        build: impl FnMut(&mut Ui) + 'static,
    ) -> Box<dyn Editor> {
        Box::new(Self {
            width,
            height,
            theme,
            params,
            build: Box::new(build),
            state: None,
            input: InputState::default(),
            mem: UiMemory::default(),
            attached_at: None,
            last_synth: None,
            pending_resize: None,
        })
    }

    fn track_mouse(&mut self, m: MouseInput) {
        self.input.mouse = (m.x, m.y);
        self.input.shift = m.shift;
    }
}

impl Editor for UiEditor {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn attached(&mut self, hwnd: *mut c_void) {
        match GpuContext::from_hwnd(hwnd, null_mut(), self.width, self.height) {
            Ok(gpu) => {
                let painter = Painter::new(&gpu);
                let text = TextRenderer::new(&gpu);
                self.state = Some(GpuState { gpu, painter, text });
                self.attached_at = Some(Instant::now());
            }
            Err(err) => eprintln!("[lantern-ui] GPU init failed: {err}"),
        }
    }

    fn removed(&mut self) {
        self.state = None;
        self.attached_at = None;
    }

    fn render(&mut self) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        state.painter.clear();
        let mut ui = Ui {
            painter: &mut state.painter,
            text: &mut state.text,
            gpu: &state.gpu,
            theme: &self.theme,
            width: state.gpu.width() as f32,
            height: state.gpu.height() as f32,
            time: self
                .attached_at
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0),
            params: self.params,
            input: &mut self.input,
            mem: &mut self.mem,
            size_request: None,
        };
        // While a dropdown popup is open it owns the mouse: the face draws
        // with clicks/wheel muted, then the overlay (drawn on top) gets the
        // real input back.
        let modal = ui.mem.dropdown.is_some();
        let stash = (
            ui.input.pressed,
            ui.input.released,
            ui.input.double_clicked,
            ui.input.wheel,
        );
        if modal {
            ui.input.pressed = false;
            ui.input.released = false;
            ui.input.double_clicked = false;
            ui.input.wheel = 0.0;
        }
        (self.build)(&mut ui);
        if modal {
            ui.input.pressed = stash.0;
            ui.input.released = stash.1;
            ui.input.double_clicked = stash.2;
            ui.input.wheel = stash.3;
        }
        ui.draw_dropdown_overlay();
        let size_request = ui.size_request;
        self.input.end_frame();
        if size_request.is_some() {
            self.pending_resize = size_request;
        }

        if state
            .painter
            .render_with_text(&state.gpu, &mut state.text, self.theme.bg)
            .is_err()
        {
            if let Some(surface) = &state.gpu.surface {
                surface.configure(&state.gpu.device, &state.gpu.config);
            }
        }
    }

    fn mouse_down(&mut self, m: MouseInput) {
        self.track_mouse(m);
        self.input.down = true;
        self.input.pressed = true;
    }

    fn mouse_up(&mut self, m: MouseInput) {
        self.track_mouse(m);
        self.input.down = false;
        self.input.released = true;
    }

    fn mouse_move(&mut self, m: MouseInput) {
        self.track_mouse(m);
    }

    fn mouse_wheel(&mut self, m: MouseInput, delta: f32) {
        self.track_mouse(m);
        self.input.wheel += delta;
    }

    fn double_click(&mut self, m: MouseInput) {
        self.track_mouse(m);
        self.input.double_clicked = true;
    }

    fn key_char(&mut self, ch: char) {
        // Drop the WM_CHAR echo of a character already synthesized from its
        // keydown (hosts that translate messages deliver both).
        if self.last_synth.take() == Some(ch) {
            return;
        }
        self.input.keys.push(crate::ui::KeyEvent::Char(ch));
    }

    fn key_down(&mut self, vk: u32) {
        self.input.keys.push(crate::ui::KeyEvent::Down(vk));
        // Hosts that dispatch raw key messages without TranslateMessage
        // produce no WM_CHAR at all; synthesize the characters a value box
        // needs (digits, sign, decimal point) straight from the keydown.
        let ch = match vk {
            0x30..=0x39 => Some((b'0' + (vk - 0x30) as u8) as char),
            0x60..=0x69 => Some((b'0' + (vk - 0x60) as u8) as char), // numpad
            0xBD | 0x6D => Some('-'),
            0xBE | 0x6E => Some('.'),
            0xBB | 0x6B => Some('+'),
            _ => None,
        };
        if let Some(ch) = ch {
            self.input.keys.push(crate::ui::KeyEvent::Char(ch));
            self.last_synth = Some(ch);
        }
    }

    fn focus_lost(&mut self) {
        crate::ui::commit_edit(&mut self.mem, self.params);
    }

    fn wants_keys(&self) -> bool {
        self.mem.edit.is_some()
    }

    fn take_resize_request(&mut self) -> Option<(u32, u32)> {
        self.pending_resize.take()
    }

    fn resized(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        if let Some(state) = self.state.as_mut() {
            state.gpu.resize(width, height);
        }
    }
}
