//! IPlugView: the editor's COM object and its child window.
//!
//! The host hands us a parent HWND in `attached`; we create a child window
//! filling the editor's size, stash the view pointer in GWLP_USERDATA, and
//! drive `Editor::render` from WM_TIMER (~60 Hz). Everything here runs on
//! the host's UI thread.

use std::cell::{Cell, UnsafeCell};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use crate::base::*;
use crate::interfaces::*;
use crate::plugin::Editor;
use crate::win32::*;

const TIMER_ID: usize = 1;

// VST3 VirtualKeyCodes (Steinberg keycodes.h) — the subset text entry needs.
const VKEY_BACK: i16 = 1;
const VKEY_RETURN: i16 = 4;
const VKEY_ESCAPE: i16 = 6;
const VKEY_UP: i16 = 12;
const VKEY_DOWN: i16 = 14;
const VKEY_ENTER: i16 = 19;
const VKEY_NUMPAD0: i16 = 24;
const VKEY_NUMPAD9: i16 = 33;
const VKEY_ADD: i16 = 35;
const VKEY_SUBTRACT: i16 = 37;
const VKEY_DECIMAL: i16 = 38;

#[repr(C)]
pub struct PlugView {
    vtbl: &'static IPlugViewVtbl,
    ref_count: AtomicU32,
    /// The plugin component (COM ref held for the view's lifetime).
    owner: *mut c_void,
    editor: UnsafeCell<Box<dyn Editor>>,
    child: Cell<HWND>,
    frame: Cell<*mut c_void>,
}

impl PlugView {
    /// Wrap an editor in a view object (refcount 1). Takes a COM reference
    /// on `owner` so the plugin instance outlives the view.
    pub fn create(owner: *mut c_void, editor: Box<dyn Editor>) -> *mut c_void {
        unsafe { addref_funknown(owner) };
        Box::into_raw(Box::new(PlugView {
            vtbl: &VTBL,
            ref_count: AtomicU32::new(1),
            owner,
            editor: UnsafeCell::new(editor),
            child: Cell::new(null_mut()),
            frame: Cell::new(null_mut()),
        })) as *mut c_void
    }

    unsafe fn me<'a>(this: *mut c_void) -> &'a PlugView {
        &*(this as *const PlugView)
    }

    unsafe fn teardown_window(&self) {
        let child = self.child.get();
        if !child.is_null() {
            KillTimer(child, TIMER_ID);
            SetWindowLongPtrW(child, GWLP_USERDATA, 0);
            (*self.editor.get()).removed();
            DestroyWindow(child);
            self.child.set(null_mut());
        }
    }

    // ------------------------------------------------------------------
    // FUnknown
    // ------------------------------------------------------------------

    unsafe extern "system" fn query_interface(
        this: *mut c_void,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if obj.is_null() {
            return kInvalidArgument;
        }
        if tuid_eq(iid, &IID_FUNKNOWN) || tuid_eq(iid, &IID_IPLUG_VIEW) {
            Self::me(this).ref_count.fetch_add(1, Ordering::Relaxed);
            *obj = this;
            kResultOk
        } else {
            *obj = null_mut();
            kNoInterface
        }
    }

    unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
        Self::me(this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn release(this: *mut c_void) -> u32 {
        let me = Self::me(this);
        let prev = me.ref_count.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            me.teardown_window();
            release_funknown(me.frame.replace(null_mut()));
            release_funknown(me.owner);
            drop(Box::from_raw(this as *mut PlugView));
            0
        } else {
            prev - 1
        }
    }

    // ------------------------------------------------------------------
    // IPlugView
    // ------------------------------------------------------------------

    unsafe extern "system" fn is_platform_type_supported(
        _this: *mut c_void,
        type_: *const i8,
    ) -> tresult {
        if is_hwnd_type(type_) {
            kResultTrue
        } else {
            kResultFalse
        }
    }

    unsafe extern "system" fn attached(
        this: *mut c_void,
        parent: *mut c_void,
        type_: *const i8,
    ) -> tresult {
        let me = Self::me(this);
        if parent.is_null() || !is_hwnd_type(type_) {
            return kInvalidArgument;
        }
        if !me.child.get().is_null() {
            return kResultFalse; // already attached
        }
        let (w, h) = (*me.editor.get()).size();
        let class_name = view_class_name();
        let child = CreateWindowExW(
            0,
            class_name.as_ptr(),
            wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            w as i32,
            h as i32,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null_mut(),
        );
        if child.is_null() {
            return kInternalError;
        }
        SetWindowLongPtrW(child, GWLP_USERDATA, this as isize);
        me.child.set(child);
        (*me.editor.get()).attached(child);
        SetTimer(child, TIMER_ID, 16, null_mut());
        kResultOk
    }

    unsafe extern "system" fn removed(this: *mut c_void) -> tresult {
        Self::me(this).teardown_window();
        kResultOk
    }

    unsafe extern "system" fn on_wheel(_this: *mut c_void, _distance: f32) -> tresult {
        kResultFalse
    }

    /// Hosts that preprocess their message pump (Ableton Live) never
    /// dispatch raw key messages to plugin child windows; they offer keys
    /// here instead. Returning kResultTrue claims the key so the host
    /// doesn't fire its own shortcut. Only claimed while a text field is
    /// active, so host shortcuts (spacebar!) keep working otherwise.
    unsafe extern "system" fn on_key_down(
        this: *mut c_void,
        key: i16,
        code: i16,
        _mods: i16,
    ) -> tresult {
        let editor = Self::me(this).editor.get();
        if !(*editor).wants_keys() {
            return kResultFalse;
        }
        // Specials first: spec says they arrive as VirtualKeyCodes, but some
        // hosts stuff the control char into `key` instead.
        let vk = match code {
            VKEY_RETURN | VKEY_ENTER => Some(0x0Du32),
            VKEY_ESCAPE => Some(0x1B),
            VKEY_BACK => Some(0x08),
            VKEY_UP => Some(0x26),
            VKEY_DOWN => Some(0x28),
            _ => match key as u16 {
                0x0D => Some(0x0D),
                0x1B => Some(0x1B),
                0x08 => Some(0x08),
                _ => None,
            },
        };
        if let Some(vk) = vk {
            (*editor).key_down(vk);
            return kResultTrue;
        }
        let ch = match code {
            VKEY_NUMPAD0..=VKEY_NUMPAD9 => Some((b'0' + (code - VKEY_NUMPAD0) as u8) as char),
            VKEY_ADD => Some('+'),
            VKEY_SUBTRACT => Some('-'),
            VKEY_DECIMAL => Some('.'),
            _ => char::from_u32(key as u16 as u32).filter(|c| !c.is_control() && *c != '\0'),
        };
        match ch {
            Some(ch) => {
                (*editor).key_char(ch);
                kResultTrue
            }
            None => kResultFalse,
        }
    }

    unsafe extern "system" fn on_key_up(
        this: *mut c_void,
        _key: i16,
        _code: i16,
        _mods: i16,
    ) -> tresult {
        // Swallow keyups while typing so the host doesn't act on them.
        if (*Self::me(this).editor.get()).wants_keys() {
            kResultTrue
        } else {
            kResultFalse
        }
    }

    unsafe extern "system" fn get_size(this: *mut c_void, size: *mut ViewRect) -> tresult {
        if size.is_null() {
            return kInvalidArgument;
        }
        let me = Self::me(this);
        let (w, h) = (*me.editor.get()).size();
        *size = ViewRect {
            left: 0,
            top: 0,
            right: w as i32,
            bottom: h as i32,
        };
        kResultOk
    }

    unsafe extern "system" fn on_size(this: *mut c_void, new_size: *mut ViewRect) -> tresult {
        if new_size.is_null() {
            return kInvalidArgument;
        }
        let me = Self::me(this);
        let w = ((*new_size).right - (*new_size).left).max(1) as u32;
        let h = ((*new_size).bottom - (*new_size).top).max(1) as u32;
        let child = me.child.get();
        if !child.is_null() {
            MoveWindow(child, 0, 0, w as i32, h as i32, 1);
        }
        (*me.editor.get()).resized(w, h);
        kResultOk
    }

    unsafe extern "system" fn on_focus(_this: *mut c_void, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe extern "system" fn set_frame(this: *mut c_void, frame: *mut c_void) -> tresult {
        let me = Self::me(this);
        addref_funknown(frame);
        release_funknown(me.frame.replace(frame));
        kResultOk
    }

    unsafe extern "system" fn can_resize(_this: *mut c_void) -> tresult {
        kResultFalse
    }

    unsafe extern "system" fn check_size_constraint(
        this: *mut c_void,
        rect: *mut ViewRect,
    ) -> tresult {
        if rect.is_null() {
            return kInvalidArgument;
        }
        let me = Self::me(this);
        let (w, h) = (*me.editor.get()).size();
        (*rect).right = (*rect).left + w as i32;
        (*rect).bottom = (*rect).top + h as i32;
        kResultTrue
    }
}

static VTBL: IPlugViewVtbl = IPlugViewVtbl {
    query_interface: PlugView::query_interface,
    add_ref: PlugView::add_ref,
    release: PlugView::release,
    is_platform_type_supported: PlugView::is_platform_type_supported,
    attached: PlugView::attached,
    removed: PlugView::removed,
    on_wheel: PlugView::on_wheel,
    on_key_down: PlugView::on_key_down,
    on_key_up: PlugView::on_key_up,
    get_size: PlugView::get_size,
    on_size: PlugView::on_size,
    on_focus: PlugView::on_focus,
    set_frame: PlugView::set_frame,
    can_resize: PlugView::can_resize,
    check_size_constraint: PlugView::check_size_constraint,
};

unsafe fn is_hwnd_type(type_: *const i8) -> bool {
    if type_.is_null() {
        return false;
    }
    for (i, &expect) in b"HWND\0".iter().enumerate() {
        if *type_.add(i) as u8 != expect {
            return false;
        }
    }
    true
}

/// Register the child window class once per module. The name embeds the
/// wndproc address so two Lantern plugin DLLs loaded into the same host
/// never fight over one class registration.
fn view_class_name() -> &'static [u16] {
    static NAME: OnceLock<Vec<u16>> = OnceLock::new();
    NAME.get_or_init(|| {
        let name = format!("LanternPlugView_{:x}", view_wndproc as usize);
        let wide_name = wide(&name);
        unsafe {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW | CS_OWNDC | CS_DBLCLKS,
                lpfnWndProc: view_wndproc,
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(null()),
                hIcon: null_mut(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW as *const u16),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: wide_name.as_ptr(),
                hIconSm: null_mut(),
            };
            RegisterClassExW(&wc);
        }
        wide_name
    })
}

/// Decode client-area mouse coordinates + current modifiers from an lparam.
unsafe fn mouse_input(lparam: LPARAM) -> crate::plugin::MouseInput {
    crate::plugin::MouseInput {
        x: (lparam & 0xFFFF) as u16 as i16 as f32,
        y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as f32,
        shift: GetKeyState(VK_SHIFT) < 0,
        ctrl: GetKeyState(VK_CONTROL) < 0,
    }
}

unsafe extern "system" fn view_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let view = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PlugView;
    if view.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let editor = (*view).editor.get();
    match msg {
        WM_TIMER => {
            // While a text edit is open, keep Win32 focus pinned here so
            // hosts that route keys by focus actually deliver them.
            if (*editor).wants_keys() && GetFocus() != hwnd {
                SetFocus(hwnd);
            }
            (*editor).render();
            // The face may have asked for a new window size this frame:
            // negotiate through the host, which answers with onSize.
            if let Some((w, h)) = (*editor).take_resize_request() {
                let frame = (*view).frame.get();
                let mut rect = ViewRect {
                    left: 0,
                    top: 0,
                    right: w as i32,
                    bottom: h as i32,
                };
                if frame.is_null() {
                    // No frame given: resize ourselves so the editor at
                    // least stays self-consistent.
                    MoveWindow(hwnd, 0, 0, w as i32, h as i32, 1);
                    (*editor).resized(w, h);
                } else {
                    let vtbl = &*(*(frame as *mut IPlugFramePtr)).vtbl;
                    (vtbl.resize_view)(frame, view as *mut c_void, &mut rect);
                }
            }
            0
        }
        WM_PAINT => {
            (*editor).render();
            ValidateRect(hwnd, null());
            0
        }
        WM_LBUTTONDOWN => {
            // Take keyboard focus so text entry works until the user clicks
            // back into the host.
            SetFocus(hwnd);
            SetCapture(hwnd);
            (*editor).mouse_down(mouse_input(lparam));
            0
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            (*editor).mouse_up(mouse_input(lparam));
            0
        }
        WM_LBUTTONDBLCLK => {
            (*editor).double_click(mouse_input(lparam));
            0
        }
        WM_MOUSEMOVE => {
            (*editor).mouse_move(mouse_input(lparam));
            0
        }
        WM_MOUSEWHEEL => {
            // Wheel coordinates arrive in screen space; convert to client.
            let delta = ((wparam >> 16) as u16 as i16) as f32 / 120.0;
            let mut pt = POINT {
                x: (lparam & 0xFFFF) as u16 as i16 as i32,
                y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32,
            };
            ScreenToClient(hwnd, &mut pt);
            let m = crate::plugin::MouseInput {
                x: pt.x as f32,
                y: pt.y as f32,
                shift: GetKeyState(VK_SHIFT) < 0,
                ctrl: GetKeyState(VK_CONTROL) < 0,
            };
            (*editor).mouse_wheel(m, delta);
            0
        }
        WM_CHAR => {
            // wparam is a UTF-16 unit; surrogates come out as None and are
            // dropped. Control chars (Enter/Esc/Backspace echoes) go through
            // WM_KEYDOWN instead.
            if let Some(ch) = char::from_u32(wparam as u32) {
                if !ch.is_control() {
                    (*editor).key_char(ch);
                }
            }
            0
        }
        WM_KEYDOWN => {
            (*editor).key_down(wparam as u32);
            0
        }
        WM_KILLFOCUS => {
            (*editor).focus_lost();
            0
        }
        WM_GETDLGCODE => DLGC_WANT_ALL,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
