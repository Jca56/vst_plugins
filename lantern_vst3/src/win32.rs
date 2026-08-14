//! Hand-declared Win32 bindings for the plugin editor's child window.
//! OS APIs are the platform, not a dependency — no winapi/windows crates.
#![allow(non_snake_case, non_camel_case_types, dead_code, clippy::too_many_arguments)]

use std::ffi::c_void;

pub type HWND = *mut c_void;
pub type HINSTANCE = *mut c_void;
pub type WPARAM = usize;
pub type LPARAM = isize;
pub type LRESULT = isize;
pub type WNDPROC = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

#[repr(C)]
pub struct WNDCLASSEXW {
    pub cbSize: u32,
    pub style: u32,
    pub lpfnWndProc: WNDPROC,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HINSTANCE,
    pub hIcon: *mut c_void,
    pub hCursor: *mut c_void,
    pub hbrBackground: *mut c_void,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
    pub hIconSm: *mut c_void,
}

pub const CS_HREDRAW: u32 = 0x0002;
pub const CS_VREDRAW: u32 = 0x0001;
pub const CS_OWNDC: u32 = 0x0020;
pub const CS_DBLCLKS: u32 = 0x0008;
pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const WM_PAINT: u32 = 0x000F;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_LBUTTONDBLCLK: u32 = 0x0203;
pub const WM_MOUSEWHEEL: u32 = 0x020A;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_CHAR: u32 = 0x0102;
pub const WM_KILLFOCUS: u32 = 0x0008;
pub const WM_GETDLGCODE: u32 = 0x0087;
/// DLGC_WANTALLKEYS | DLGC_WANTCHARS: hosts that route keys through
/// IsDialogMessage must hand us everything.
pub const DLGC_WANT_ALL: isize = 0x0004 | 0x0080;
pub const VK_SHIFT: i32 = 0x10;
pub const VK_CONTROL: i32 = 0x11;
pub const GWLP_USERDATA: i32 = -21;
pub const IDC_ARROW: usize = 32512;

#[repr(C)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[link(name = "user32")]
extern "system" {
    pub fn RegisterClassExW(class: *const WNDCLASSEXW) -> u16;
    pub fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: HWND,
        menu: *mut c_void,
        instance: HINSTANCE,
        param: *mut c_void,
    ) -> HWND;
    pub fn DestroyWindow(hwnd: HWND) -> i32;
    pub fn DefWindowProcW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    pub fn SetTimer(hwnd: HWND, id: usize, elapse_ms: u32, callback: *mut c_void) -> usize;
    pub fn KillTimer(hwnd: HWND, id: usize) -> i32;
    pub fn LoadCursorW(instance: HINSTANCE, name: *const u16) -> *mut c_void;
    pub fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    pub fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
    pub fn ValidateRect(hwnd: HWND, rect: *const c_void) -> i32;
    pub fn SetCapture(hwnd: HWND) -> HWND;
    pub fn ReleaseCapture() -> i32;
    pub fn SetFocus(hwnd: HWND) -> HWND;
    pub fn GetFocus() -> HWND;
    pub fn MoveWindow(hwnd: HWND, x: i32, y: i32, w: i32, h: i32, repaint: i32) -> i32;
    pub fn GetKeyState(key: i32) -> i16;
    pub fn ScreenToClient(hwnd: HWND, point: *mut POINT) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
