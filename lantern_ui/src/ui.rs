//! Immediate-mode widgets. Each widget call both draws and handles input:
//! events accumulate in `InputState` between frames, widgets consume them
//! during `render`, and every value change flows through
//! `ParamsHandle::begin/perform/end_edit` so the host records automation.

use std::f32::consts::PI;

use lantern_gpu::{Color, GpuContext, Painter, Rect};
use lantern_vst3::plugin::ParamsHandle;

use crate::text::TextRenderer;
use crate::theme::Theme;

/// One keyboard event, in arrival order (chars and raw keys interleave).
#[derive(Clone, Copy)]
pub enum KeyEvent {
    /// Translated printable character.
    Char(char),
    /// Windows virtual-key code (0x0D Enter, 0x1B Esc, 0x08 Backspace, ...).
    Down(u32),
}

pub(crate) const VK_BACK: u32 = 0x08;
pub(crate) const VK_RETURN: u32 = 0x0D;
pub(crate) const VK_ESCAPE: u32 = 0x1B;
pub(crate) const VK_UP: u32 = 0x26;
pub(crate) const VK_DOWN: u32 = 0x28;

/// Mouse + keyboard state accumulated between frames by the editor shell.
#[derive(Default)]
pub struct InputState {
    pub mouse: (f32, f32),
    pub down: bool,
    pub pressed: bool,
    pub released: bool,
    pub double_clicked: bool,
    pub wheel: f32,
    pub shift: bool,
    pub keys: Vec<KeyEvent>,
}

impl InputState {
    pub(crate) fn end_frame(&mut self) {
        self.pressed = false;
        self.released = false;
        self.double_clicked = false;
        self.wheel = 0.0;
        self.keys.clear();
    }
}

/// A value box being typed into.
pub(crate) struct EditBox {
    id: u32,
    param: usize,
    text: String,
    /// Still showing the seeded value: the first keystroke replaces it
    /// (select-all-on-click behavior without a selection model).
    pristine: bool,
}

/// Cross-frame widget state (which widget owns the current drag).
#[derive(Default)]
pub struct UiMemory {
    active: Option<u32>,
    drag_anchor_y: f32,
    drag_anchor_value: f64,
    drag_fine: bool,
    /// Whether the active drag has moved past the click threshold
    /// (drag boxes: a motionless click opens typed entry instead).
    drag_moved: bool,
    /// Smoothed display values for meters (needle ballistics).
    meter_smooth: [f64; 8],
    pub(crate) edit: Option<EditBox>,
}

/// Parse a typed value: leading float, then an optional L/R suffix
/// (balance-style, L = negative). Rejects anything with no number.
fn parse_value(s: &str) -> Option<f64> {
    let s = s.trim();
    let mut end = 0;
    for c in s.chars() {
        let numeric = c.is_ascii_digit() || c == '.' || ((c == '+' || c == '-') && end == 0);
        if !numeric {
            break;
        }
        end += c.len_utf8();
    }
    let mut v: f64 = s[..end].parse().ok()?;
    let rest = s[end..].trim_start();
    if rest.starts_with(['L', 'l']) {
        v = -v;
    } else if rest.starts_with(['k', 'K']) {
        // "1.2k" for frequencies.
        v *= 1000.0;
    }
    Some(v)
}

/// Commit any pending text entry: parse, map through the param's plain
/// range, and hand it to the host as a normal edit gesture. Unparseable
/// text just reverts. Called by the value box (Enter / click-away) and by
/// the editor shell when the window loses keyboard focus.
pub(crate) fn commit_edit(mem: &mut UiMemory, params: ParamsHandle) {
    let Some(edit) = mem.edit.take() else { return };
    let Some(plain) = parse_value(&edit.text) else { return };
    let normalized = params.def(edit.param).normalized_from_plain(plain);
    params.begin_edit(edit.param);
    params.perform_edit(edit.param, normalized);
    params.end_edit(edit.param);
}

/// One frame's UI context.
pub struct Ui<'a> {
    pub painter: &'a mut Painter,
    pub text: &'a mut TextRenderer,
    pub gpu: &'a GpuContext,
    pub theme: &'a Theme,
    pub width: f32,
    pub height: f32,
    pub time: f32,
    pub params: ParamsHandle,
    pub(crate) input: &'a mut InputState,
    pub(crate) mem: &'a mut UiMemory,
    pub(crate) size_request: Option<(u32, u32)>,
}

/// Full-range drag distance in pixels (shift = 10x finer).
const DRAG_RANGE_PX: f64 = 200.0;
const FINE_FACTOR: f64 = 10.0;

impl Ui<'_> {
    // ------------------------------------------------------------------
    // Chrome helpers
    // ------------------------------------------------------------------

    /// Standard Lantern face chrome: the panel fills the window. No title,
    /// no banner — Alva knows what the plugin is called; every pixel works.
    /// Returns the panel rect to lay widgets out in.
    pub fn face(&mut self) -> Rect {
        let t = self.theme;
        let panel = Rect::new(14.0, 14.0, self.width - 28.0, self.height - 28.0);
        self.painter
            .shadow(panel, 10.0, 10.0, Color::rgba(0.0, 0.0, 0.0, 0.5), 0.0, 5.0);
        self.painter.rect_filled(panel, 10.0, t.panel);
        self.painter.rect_border(panel, 10.0, 3.0, t.panel_border);
        panel
    }

    /// Thin Lantern gradient hairline — the last surviving gradient.
    /// Fades out at both ends: dark into the panel on the left, orange
    /// dissolving to transparent on the right.
    pub fn divider(&mut self, x: f32, y: f32, w: f32) {
        let t = self.theme;
        self.painter.rect_gradient_multi_direct(
            Rect::new(x, y, w, 3.0),
            0.0,
            0.0,
            &[
                t.header_a,
                t.header_b,
                t.header_c,
                t.header_c.with_alpha(0.0),
            ],
        );
    }

    pub fn label(&mut self, text: &str, x: f32, y: f32, px: f32, color: Color) {
        self.text.queue(self.gpu, text, x, y, px, color);
    }

    pub fn label_centered(&mut self, text: &str, cx: f32, y: f32, px: f32, color: Color) {
        self.text.queue_centered(self.gpu, text, cx, y, px, color);
    }

    // ------------------------------------------------------------------
    // Shared interaction logic
    // ------------------------------------------------------------------

    /// Drag/click/wheel handling shared by knob and slider.
    /// `id` must be unique per widget (param index works).
    fn interact(&mut self, id: u32, param: usize, hovered: bool) {
        let input = &mut *self.input;
        let mem = &mut *self.mem;

        if input.pressed && hovered && mem.active.is_none() {
            mem.active = Some(id);
            mem.drag_anchor_y = input.mouse.1;
            mem.drag_anchor_value = self.params.normalized(param);
            mem.drag_fine = input.shift;
            self.params.begin_edit(param);
        }

        if mem.active == Some(id) {
            // Re-anchor when the fine modifier toggles mid-drag, so the
            // value doesn't jump.
            if input.shift != mem.drag_fine {
                mem.drag_fine = input.shift;
                mem.drag_anchor_y = input.mouse.1;
                mem.drag_anchor_value = self.params.normalized(param);
            }
            let range = if mem.drag_fine {
                DRAG_RANGE_PX * FINE_FACTOR
            } else {
                DRAG_RANGE_PX
            };
            let delta = (mem.drag_anchor_y - input.mouse.1) as f64 / range;
            self.params
                .perform_edit(param, mem.drag_anchor_value + delta);
            if input.released {
                self.params.end_edit(param);
                mem.active = None;
            }
        } else if hovered {
            if input.double_clicked {
                let default = self.params.def(param).default_normalized;
                self.params.begin_edit(param);
                self.params.perform_edit(param, default);
                self.params.end_edit(param);
            } else if input.wheel != 0.0 {
                let step = if input.shift { 0.002 } else { 0.02 };
                let value = self.params.normalized(param) + step * input.wheel as f64;
                self.params.begin_edit(param);
                self.params.perform_edit(param, value);
                self.params.end_edit(param);
            }
        }
    }

    fn is_active(&self, id: u32) -> bool {
        self.mem.active == Some(id)
    }

    // ------------------------------------------------------------------
    // Widgets
    // ------------------------------------------------------------------

    /// Rotary knob bound to `param`, centered at (cx, cy).
    /// Label above, live value readout below. Drag vertically; shift = fine;
    /// double-click = reset to default; wheel = step.
    pub fn knob(&mut self, param: usize, cx: f32, cy: f32, radius: f32, label: &str) {
        self.knob_framed(param, cx, cy, radius, radius, label);
    }

    /// Knob cluster placed in a cell: label on the cell's top edge, value
    /// box on its bottom edge, knob centered between. Cells with equal tops
    /// and bottoms produce aligned rows by construction, whatever the knob
    /// sizes — the text frame derives from the cell height and the radius
    /// fills it, capped by the cell width.
    pub fn knob_cell(&mut self, param: usize, cell: Rect, label: &str) {
        let frame = ((cell.h - 86.0) * 0.5).max(20.0);
        let radius = frame.min(cell.w * 0.5);
        self.knob_framed(
            param,
            cell.center_x(),
            cell.y + frame + 40.0,
            radius,
            frame,
            label,
        );
    }

    /// `knob`, but with the label and value box placed as if the knob had
    /// `frame` radius — differently-sized knobs on one row keep their text
    /// on shared lines.
    pub fn knob_framed(
        &mut self,
        param: usize,
        cx: f32,
        cy: f32,
        radius: f32,
        frame: f32,
        label: &str,
    ) {
        let id = param as u32;
        let (mx, my) = self.input.mouse;
        let hovered = {
            let (dx, dy) = (mx - cx, my - cy);
            (dx * dx + dy * dy).sqrt() <= radius + 10.0
        };
        self.interact(id, param, hovered);
        let active = self.is_active(id);

        let value = self.params.normalized(param) as f32;
        let start = 0.75 * PI;
        let range = 1.5 * PI;
        let track_w = (radius * 0.12).clamp(4.0, 8.0);
        let t = self.theme;

        // Soft halo when engaged (a real falloff glow this time).
        if hovered || active {
            let halo = radius * 2.6;
            self.painter.rect_radial_glow(
                Rect::new(cx - halo, cy - halo, halo * 2.0, halo * 2.0),
                halo,
                (0.5, 0.5),
                halo,
                t.accent.lighten(0.5).with_alpha(if active { 0.08 } else { 0.04 }),
            );
        }

        // Body, track, value arc.
        self.painter
            .circle_filled(cx, cy, radius - track_w - 6.0, t.well);
        self.painter.arc(cx, cy, radius, start, range, track_w, 0.0, t.track);
        if value > 0.001 {
            self.painter.arc(
                cx,
                cy,
                radius,
                start,
                range * value,
                track_w,
                0.0,
                if active { t.warm } else { t.accent },
            );
        }

        // Default-value tick (12 o'clock for centered params).
        let dt = self.params.def(param).default_normalized as f32;
        let tick = start + range * dt;
        self.painter.line(
            cx + tick.cos() * (radius + 4.0),
            cy + tick.sin() * (radius + 4.0),
            cx + tick.cos() * (radius + 9.0),
            cy + tick.sin() * (radius + 9.0),
            2.0,
            t.text_dim,
        );

        // Needle + cap.
        let angle = start + range * value;
        self.painter.line(
            cx,
            cy,
            cx + angle.cos() * (radius - track_w - 10.0),
            cy + angle.sin() * (radius - track_w - 10.0),
            3.0,
            t.needle,
        );
        self.painter.circle_filled(cx, cy, 5.0, t.needle);

        // Label above, click-to-type value box below (on the frame's rows).
        self.label_centered(label, cx, cy - frame - 40.0, 21.0, t.text_dim);
        self.value_box(param, Rect::new(cx - 56.0, cy + frame + 12.0, 112.0, 34.0));
    }

    /// Click-to-type numeric readout bound to `param`. Shows the formatted
    /// value; clicking seeds an edit buffer (first keystroke replaces it,
    /// select-all style), Enter or clicking away commits, Esc reverts,
    /// Up/Down nudges, and unparseable input reverts silently.
    pub fn value_box(&mut self, param: usize, rect: Rect) {
        let id = 0x2000 + param as u32;
        let (mx, my) = self.input.mouse;
        let hovered = rect.contains(mx, my);
        if self.input.pressed {
            let editing_this = matches!(&self.mem.edit, Some(e) if e.id == id);
            if hovered && !editing_this {
                self.open_edit(id, param);
            } else if !hovered && editing_this {
                commit_edit(self.mem, self.params);
            }
        }
        self.edit_keys(id);
        self.edit_chrome(id, param, rect, hovered, None);
    }

    /// Number box adjusted by horizontal drag (shift = fine, wheel steps);
    /// a click without movement opens typed entry, like the value box.
    pub fn drag_box(&mut self, param: usize, rect: Rect) {
        let id = 0x3000 + param as u32;
        let (mx, _my) = self.input.mouse;
        let hovered = rect.contains(self.input.mouse.0, self.input.mouse.1);
        let editing_this = matches!(&self.mem.edit, Some(e) if e.id == id);

        if self.input.pressed {
            if !hovered && editing_this {
                commit_edit(self.mem, self.params);
            }
            if hovered && !editing_this && self.mem.active.is_none() {
                self.mem.active = Some(id);
                self.mem.drag_anchor_y = mx; // horizontal drag: anchor holds x
                self.mem.drag_anchor_value = self.params.normalized(param);
                self.mem.drag_fine = self.input.shift;
                self.mem.drag_moved = false;
                self.params.begin_edit(param);
            }
        }
        if self.mem.active == Some(id) {
            if self.input.shift != self.mem.drag_fine {
                self.mem.drag_fine = self.input.shift;
                self.mem.drag_anchor_y = mx;
                self.mem.drag_anchor_value = self.params.normalized(param);
            }
            let range = if self.mem.drag_fine {
                DRAG_RANGE_PX * FINE_FACTOR
            } else {
                DRAG_RANGE_PX
            };
            let dx = (mx - self.mem.drag_anchor_y) as f64;
            if dx.abs() > 3.0 {
                self.mem.drag_moved = true;
            }
            if self.mem.drag_moved {
                self.params
                    .perform_edit(param, self.mem.drag_anchor_value + dx / range);
            }
            if self.input.released {
                self.params.end_edit(param);
                self.mem.active = None;
                if !self.mem.drag_moved {
                    self.open_edit(id, param);
                }
            }
        } else if hovered && self.input.wheel != 0.0 {
            let step = if self.input.shift { 0.002 } else { 0.02 };
            let value = self.params.normalized(param) + step * self.input.wheel as f64;
            self.params.begin_edit(param);
            self.params.perform_edit(param, value);
            self.params.end_edit(param);
        }

        self.edit_keys(id);
        let engaged = hovered || self.mem.active == Some(id);
        let fill = self.params.normalized(param) as f32;
        self.edit_chrome(id, param, rect, engaged, Some(fill));
    }

    /// Momentary action button; true on the frame it's clicked.
    /// Disabled buttons render dim and ignore the mouse.
    pub fn button(&mut self, rect: Rect, label: &str, enabled: bool) -> bool {
        let (mx, my) = self.input.mouse;
        let hovered = enabled && rect.contains(mx, my);
        let t = self.theme;
        self.painter
            .rect_filled(rect, 6.0, if hovered { t.track } else { t.well });
        self.painter.rect_border(
            rect,
            6.0,
            2.5,
            if hovered { t.text_dim } else { t.panel_border },
        );
        let color = if !enabled {
            t.text_dim.with_alpha(0.5)
        } else if hovered {
            t.text
        } else {
            t.text_dim
        };
        let w = self.text.measure(label, 21.0);
        self.label(
            label,
            rect.center_x() - w * 0.5,
            rect.center_y() - self.text.line_height(21.0) * 0.5 + 1.0,
            21.0,
            color,
        );
        hovered && self.input.pressed
    }

    /// Open a typed-entry session on `param`, committing any other pending
    /// edit first. The seeded text is "selected": the first keystroke eats it.
    fn open_edit(&mut self, id: u32, param: usize) {
        commit_edit(self.mem, self.params);
        self.mem.edit = Some(EditBox {
            id,
            param,
            text: self.params.def(param).display(self.params.normalized(param)),
            pristine: true,
        });
    }

    /// Consume this frame's keyboard events for the edit box owned by `id`.
    fn edit_keys(&mut self, id: u32) {
        // Keyboard, if this box owns the edit.
        let mut commit = false;
        let mut cancel = false;
        if let Some(edit) = self.mem.edit.as_mut().filter(|e| e.id == id) {
            let param = edit.param;
            for &ev in self.input.keys.iter() {
                match ev {
                    KeyEvent::Char(c) if c.is_ascii_graphic() => {
                        if edit.pristine {
                            edit.text.clear();
                            edit.pristine = false;
                        }
                        if edit.text.len() < 12 {
                            edit.text.push(c);
                        }
                    }
                    KeyEvent::Down(VK_BACK) => {
                        if edit.pristine {
                            edit.text.clear();
                            edit.pristine = false;
                        } else {
                            edit.text.pop();
                        }
                    }
                    KeyEvent::Down(VK_RETURN) => commit = true,
                    KeyEvent::Down(VK_ESCAPE) => cancel = true,
                    KeyEvent::Down(vk @ (VK_UP | VK_DOWN)) => {
                        // Nudge from whatever is typed (or the live value if
                        // the text doesn't parse), apply immediately, and
                        // reseed the box so the next digit replaces it.
                        let def = self.params.def(param);
                        let dir = if vk == VK_UP { 1.0 } else { -1.0 };
                        let cur = parse_value(&edit.text)
                            .unwrap_or_else(|| def.plain(self.params.normalized(param)));
                        let normalized = if def.step_count > 0 {
                            (def.normalized_from_plain(cur) + dir / def.step_count as f64)
                                .clamp(0.0, 1.0)
                        } else {
                            // One plain unit per notch; shift = a tenth.
                            let step = if self.input.shift { 0.1 } else { 1.0 };
                            def.normalized_from_plain(cur + dir * step)
                        };
                        self.params.begin_edit(param);
                        self.params.perform_edit(param, normalized);
                        self.params.end_edit(param);
                        edit.text = def.display(normalized);
                        edit.pristine = true;
                    }
                    _ => {}
                }
            }
        }
        if cancel {
            self.mem.edit = None;
        } else if commit {
            commit_edit(self.mem, self.params);
        }
    }

    /// Draw a value/drag box: chrome, current text or edit buffer, caret.
    /// `fill` paints the box like a slider — empty at 0, full at 1.
    fn edit_chrome(&mut self, id: u32, param: usize, rect: Rect, hovered: bool, fill: Option<f32>) {
        let t = self.theme;
        let editing = matches!(&self.mem.edit, Some(e) if e.id == id);
        self.painter.rect_filled(
            rect,
            6.0,
            if editing { t.well_deep } else { t.well },
        );
        if let Some(f) = fill {
            if !editing && f > 0.01 {
                self.painter.push_clip(rect);
                self.painter.rect_filled(
                    Rect::new(rect.x, rect.y, rect.w * f.clamp(0.0, 1.0), rect.h),
                    6.0,
                    t.accent.with_alpha(0.30),
                );
                self.painter.pop_clip();
            }
        }
        let border = if editing {
            t.accent
        } else if hovered {
            t.text_dim
        } else {
            t.panel_border
        };
        self.painter.rect_border(rect, 6.0, 2.5, border);

        let (shown, pristine) = match &self.mem.edit {
            Some(e) if e.id == id => (e.text.clone(), e.pristine),
            _ => {
                let def = self.params.def(param);
                let readout = format!("{} {}", def.display(self.params.normalized(param)), def.units);
                (readout.trim_end().to_string(), false)
            }
        };
        let px = 21.0;
        let w = self.text.measure(&shown, px);
        let tx = rect.center_x() - w * 0.5;
        let ty = rect.center_y() - self.text.line_height(px) * 0.5 + 1.0;
        if pristine && !shown.is_empty() {
            // The seeded value reads as selected: the next keystroke eats it.
            self.painter.rect_filled(
                Rect::new(tx - 3.0, rect.y + 5.0, w + 6.0, rect.h - 10.0),
                3.0,
                t.accent.with_alpha(0.22),
            );
        }
        self.label(&shown, tx, ty, px, t.text);
        if editing && (self.time * 2.0).fract() < 0.6 {
            let caret_x = tx + w + 2.0;
            self.painter.line(
                caret_x,
                rect.y + 7.0,
                caret_x,
                rect.y + rect.h - 7.0,
                2.0,
                t.text,
            );
        }
    }

    /// Horizontal slider bound to `param`.
    pub fn hslider(&mut self, param: usize, rect: Rect, label: &str) {
        let id = 0x1000 + param as u32;
        let (mx, my) = self.input.mouse;
        let hovered = rect.expand(6.0).contains(mx, my);
        self.interact(id, param, hovered);
        let active = self.is_active(id);

        let value = self.params.normalized(param) as f32;
        let t = self.theme;
        let track_h = 6.0;
        let track = Rect::new(
            rect.x,
            rect.center_y() - track_h * 0.5,
            rect.w,
            track_h,
        );
        self.painter.rect_filled(track, 3.0, t.track);
        self.painter.rect_filled(
            Rect::new(track.x, track.y, track.w * value, track_h),
            3.0,
            if active { t.warm } else { t.accent },
        );
        let hx = track.x + track.w * value;
        self.painter.circle_filled(hx, rect.center_y(), 9.0, t.needle);

        self.label(label, rect.x, rect.y - 30.0, 21.0, t.text_dim);
        let def = self.params.def(param);
        let readout = format!("{} {}", def.display(value as f64), def.units);
        let w = self.text.measure(&readout, 21.0);
        self.label(&readout, rect.x + rect.w - w, rect.y - 30.0, 21.0, t.text);
    }

    /// Toggle button bound to a step_count == 1 param.
    pub fn toggle(&mut self, param: usize, rect: Rect, label: &str) {
        let (mx, my) = self.input.mouse;
        let hovered = rect.contains(mx, my);
        let on = self.params.normalized(param) >= 0.5;
        if hovered && self.input.pressed {
            self.params.begin_edit(param);
            self.params.perform_edit(param, if on { 0.0 } else { 1.0 });
            self.params.end_edit(param);
        }

        let t = self.theme;
        self.painter.rect_filled(rect, 6.0, if on { t.accent.with_alpha(0.25) } else { t.well });
        self.painter.rect_border(
            rect,
            6.0,
            2.5,
            if on { t.accent } else { t.panel_border },
        );
        let color = if on { t.text } else { t.text_dim };
        let w = self.text.measure(label, 21.0);
        self.label(
            label,
            rect.center_x() - w * 0.5,
            rect.center_y() - self.text.line_height(21.0) * 0.5 + 1.0,
            21.0,
            color,
        );
    }

    /// One option of a mutually-exclusive set bound to a stepped param
    /// (`step_count >= 1`). Lit when the param currently sits on `step`;
    /// clicking selects it. Radio rows (root/scale pickers) are loops of
    /// these.
    pub fn choice(&mut self, param: usize, step: i32, rect: Rect, label: &str) {
        let (mx, my) = self.input.mouse;
        let hovered = rect.contains(mx, my);
        let steps = self.params.def(param).step_count.max(1);
        let current = (self.params.normalized(param) * steps as f64).round() as i32;
        let on = current == step;
        if hovered && self.input.pressed && !on {
            self.params.begin_edit(param);
            self.params.perform_edit(param, step as f64 / steps as f64);
            self.params.end_edit(param);
        }

        let t = self.theme;
        self.painter
            .rect_filled(rect, 6.0, if on { t.accent.with_alpha(0.25) } else { t.well_deep });
        let border = if on {
            t.accent
        } else if hovered {
            t.text_dim
        } else {
            t.panel_border
        };
        self.painter.rect_border(rect, 6.0, 2.5, border);
        let color = if on { t.text } else { t.text_dim };
        let w = self.text.measure(label, 21.0);
        self.label(
            label,
            rect.center_x() - w * 0.5,
            rect.center_y() - self.text.line_height(21.0) * 0.5 + 1.0,
            21.0,
            color,
        );
    }

    /// Ask the host to resize the editor window (negotiated after this
    /// frame; the face sees the new size once the host agrees). Draw from
    /// `self.width`/`self.height`, not from what was requested.
    pub fn request_size(&mut self, width: u32, height: u32) {
        self.size_request = Some((width, height));
    }

    /// Mouse position this frame (editor-local pixels), for widgets a plugin
    /// face draws itself.
    pub fn mouse_pos(&self) -> (f32, f32) {
        self.input.mouse
    }

    /// True only on the frame the left button went down.
    pub fn mouse_clicked(&self) -> bool {
        self.input.pressed
    }

    /// True while the left button is held.
    pub fn mouse_is_down(&self) -> bool {
        self.input.down
    }

    /// True only on the frame the left button was released.
    pub fn mouse_released(&self) -> bool {
        self.input.released
    }

    /// This frame's scroll-wheel notches (+ = away from the user).
    pub fn wheel_delta(&self) -> f32 {
        self.input.wheel
    }

    /// Vertical stereo peak meter with an RMS core and a held-peak readout.
    ///
    /// `levels`, `peaks`, and `rms` are meter-slot pairs (L, R), linear
    /// amplitude: `levels` carry the DSP's fall-ballistics peak envelope,
    /// `peaks` a max-since-reset the DSP maintains read-modify-write, and
    /// `rms` a windowed RMS. Bars run green up to 0 dBFS and red above,
    /// with the RMS drawn as a brighter core inside the peak bar — bar top
    /// answers "will it clip", core answers "how loud does it feel". Thin
    /// cross-lines mark the held peaks; the readout under the bars shows
    /// the loudest held peak in dB and zeroes both peak slots when clicked.
    pub fn level_meter(
        &mut self,
        rect: Rect,
        levels: (usize, usize),
        peaks: (usize, usize),
        rms: (usize, usize),
    ) {
        const DB_MIN: f32 = -60.0;
        const DB_MAX: f32 = 6.0;
        let t = self.theme;

        let db = |v: f64| 20.0 * (v as f32).max(1e-7).log10();
        let lv = [db(self.params.meter(levels.0)), db(self.params.meter(levels.1))];
        let pk = [db(self.params.meter(peaks.0)), db(self.params.meter(peaks.1))];
        let rm = [db(self.params.meter(rms.0)), db(self.params.meter(rms.1))];

        // Bars in a well on top, readout box below (readout gets a minimum
        // width so "+10.5 dB" never bursts a narrow meter's seams).
        let readout_h = 32.0;
        let well = Rect::new(rect.x, rect.y, rect.w, rect.h - readout_h - 12.0);
        let readout_w = rect.w.max(96.0);
        let readout = Rect::new(
            rect.center_x() - readout_w * 0.5,
            rect.y + rect.h - readout_h,
            readout_w,
            readout_h,
        );

        self.painter.rect_filled(well, 10.0, t.well_deep);
        self.painter
            .inner_shadow(well, 10.0, 8.0, Color::rgba(0.0, 0.0, 0.0, 0.5), 0.0, 3.0);
        self.painter.rect_border(well, 10.0, 3.0, t.panel_border);

        let inset = 10.0;
        let label_h = 26.0;
        let bars = Rect::new(
            well.x + inset,
            well.y + inset + label_h,
            well.w - 2.0 * inset,
            well.h - 2.0 * inset - label_h,
        );
        let bar_w = (bars.w - 6.0) * 0.5;
        let bar_x = [bars.x, bars.x + bar_w + 6.0];
        let y_of = |db: f32| -> f32 {
            let f = ((db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0);
            bars.y + (1.0 - f) * bars.h
        };

        for (i, ch) in ["L", "R"].iter().enumerate() {
            self.label_centered(ch, bar_x[i] + bar_w * 0.5, well.y + 8.0, 18.0, t.text_dim);
        }

        // Scale: dim hairlines every 12 dB, a firm line at 0 dBFS.
        for tick in [-48.0, -36.0, -24.0, -12.0] {
            let y = y_of(tick);
            self.painter.line(bars.x, y, bars.x + bars.w, y, 1.0, t.track);
        }
        let y0 = y_of(0.0);
        self.painter.line(bars.x, y0, bars.x + bars.w, y0, 2.0, t.text_dim);

        for i in 0..2 {
            let x = bar_x[i];
            self.painter.rect_filled(
                Rect::new(x, bars.y, bar_w, bars.h),
                2.0,
                t.track.lighten(0.16),
            );
            if lv[i] > DB_MIN {
                let bottom = bars.y + bars.h;
                let green_top = y_of(lv[i].min(0.0));
                self.painter.rect_filled(
                    Rect::new(x, green_top, bar_w, bottom - green_top),
                    2.0,
                    t.meter_ok,
                );
                if lv[i] > 0.0 {
                    let red_top = y_of(lv[i]);
                    self.painter.rect_filled(
                        Rect::new(x, red_top, bar_w, y0 - red_top),
                        2.0,
                        t.meter_hot,
                    );
                }
            }
            // RMS core: a narrower, brighter column inside the peak bar.
            if rm[i] > DB_MIN {
                let core_w = bar_w * 0.44;
                let core_x = x + (bar_w - core_w) * 0.5;
                let bottom = bars.y + bars.h;
                let core_top = y_of(rm[i].min(0.0));
                self.painter.rect_filled(
                    Rect::new(core_x, core_top, core_w, bottom - core_top),
                    2.0,
                    t.meter_ok.lighten(0.5),
                );
                if rm[i] > 0.0 {
                    let red_top = y_of(rm[i]);
                    self.painter.rect_filled(
                        Rect::new(core_x, red_top, core_w, y0 - red_top),
                        2.0,
                        t.meter_hot.lighten(0.4),
                    );
                }
            }
            if pk[i] > DB_MIN {
                let y = y_of(pk[i]);
                let c = if pk[i] >= 0.0 { t.meter_hot } else { t.meter_ok };
                self.painter.line(x, y, x + bar_w, y, 2.0, c);
            }
        }

        // Held-peak readout; click resets.
        let (mx, my) = self.input.mouse;
        let hovered = readout.contains(mx, my);
        if hovered && self.input.pressed {
            self.params.meter_set(peaks.0, 0.0);
            self.params.meter_set(peaks.1, 0.0);
        }
        let peak_db = pk[0].max(pk[1]);
        let clipped = peak_db >= 0.0;
        self.painter.rect_filled(readout, 6.0, t.well);
        let border = if clipped {
            t.meter_hot
        } else if hovered {
            t.text_dim
        } else {
            t.panel_border
        };
        self.painter.rect_border(readout, 6.0, 2.5, border);
        let text = if peak_db <= -90.0 {
            "-∞ dB".to_string()
        } else {
            format!("{peak_db:+.1} dB")
        };
        let color = if clipped { t.meter_hot } else { t.text };
        let w = self.text.measure(&text, 19.0);
        self.label(
            &text,
            readout.center_x() - w * 0.5,
            readout.center_y() - self.text.line_height(19.0) * 0.5 + 1.0,
            19.0,
            color,
        );
    }

    /// Analog-style gain-reduction needle meter.
    ///
    /// `slot` picks a ballistics slot in `UiMemory` (smoothing state);
    /// `gr_db` is the current reduction in dB (>= 0, more = harder squash).
    /// The needle snaps down fast and recovers slow, VU-style, and the well
    /// glows warmer the harder the compressor works.
    pub fn gr_meter(&mut self, slot: usize, gr_db: f64, rect: Rect) {
        const MAX_DB: f64 = 20.0;

        // Ballistics: fast attack, slow release, framerate-tied (fine at 60).
        let shown = {
            let s = &mut self.mem.meter_smooth[slot];
            let target = gr_db.clamp(0.0, MAX_DB);
            let coef = if target > *s { 0.5 } else { 0.06 };
            *s += (target - *s) * coef;
            *s
        };
        let heat = (shown / MAX_DB) as f32;

        let t = self.theme;

        // The well.
        self.painter.rect_filled(rect, 12.0, t.well_deep);
        self.painter.inner_shadow(
            rect,
            12.0,
            10.0,
            Color::rgba(0.0, 0.0, 0.0, 0.5),
            0.0,
            4.0,
        );
        self.painter.rect_border(rect, 12.0, 3.0, t.panel_border);

        // Lantern heat: glows from the bottom as reduction increases.
        if heat > 0.01 {
            self.painter.rect_radial_glow(
                rect,
                12.0,
                (0.5, 1.0),
                rect.h * 1.1,
                t.warm.with_alpha(0.22 * heat),
            );
        }

        self.label_centered(
            "GAIN REDUCTION",
            rect.center_x(),
            rect.y + 16.0,
            20.0,
            t.text_dim,
        );

        // Scale geometry: needle pivot sits below the well; the arc sweeps
        // big across the upper half, ±42° around vertical. 0 dB left, MAX
        // right, the readout beneath the arc.
        let pivot = (rect.center_x(), rect.y + rect.h * 1.05);
        let r_scale = rect.h * 0.75;
        let half_sweep = 42f32.to_radians();
        let angle_of = |db: f64| -> f32 {
            let frac = (db / MAX_DB) as f32;
            -std::f32::consts::FRAC_PI_2 - half_sweep + frac * 2.0 * half_sweep
        };

        // Tick band arc.
        self.painter.arc(
            pivot.0,
            pivot.1,
            r_scale - 14.0,
            angle_of(0.0),
            2.0 * half_sweep,
            2.5,
            0.0,
            t.track,
        );

        // Ticks + numbers.
        for db in [0.0, 5.0, 10.0, 15.0, 20.0] {
            let a = angle_of(db);
            let (dx, dy) = (a.cos(), a.sin());
            self.painter.line(
                pivot.0 + dx * (r_scale - 10.0),
                pivot.1 + dy * (r_scale - 10.0),
                pivot.0 + dx * (r_scale + 4.0),
                pivot.1 + dy * (r_scale + 4.0),
                2.0,
                t.text_dim,
            );
            let label = format!("{}", db as i32);
            self.label_centered(
                &label,
                pivot.0 + dx * (r_scale + 22.0),
                pivot.1 + dy * (r_scale + 22.0) - 10.0,
                18.0,
                t.text_dim,
            );
        }

        // Needle, clipped to the well.
        self.painter.push_clip(rect.expand(-3.0));
        let a = angle_of(shown);
        self.painter.line(
            pivot.0,
            pivot.1,
            pivot.0 + a.cos() * (r_scale - 18.0),
            pivot.1 + a.sin() * (r_scale - 18.0),
            3.5,
            if heat > 0.5 { t.warm } else { t.accent },
        );
        self.painter.pop_clip();

        // Numeric readout.
        let readout = if shown < 0.05 {
            "0.0 dB".to_string()
        } else {
            format!("-{shown:.1} dB")
        };
        self.label_centered(
            &readout,
            rect.center_x(),
            rect.y + rect.h * 0.76,
            44.0,
            t.text,
        );
    }
}
