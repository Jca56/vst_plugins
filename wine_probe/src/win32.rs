//! Hand-declared Win32 bindings — just the handful of functions a probe
//! window needs. No winapi/windows crates: OS APIs are the platform, not a
//! dependency. These declarations will graduate into the plugin editor
//! window code in later phases.
#![allow(non_snake_case, non_camel_case_types, dead_code, clippy::too_many_arguments)]

use std::ffi::c_void;

pub type HWND = *mut c_void;
pub type HINSTANCE = *mut c_void;
pub type HICON = *mut c_void;
pub type HCURSOR = *mut c_void;
pub type HBRUSH = *mut c_void;
pub type HMENU = *mut c_void;
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
    pub hIcon: HICON,
    pub hCursor: HCURSOR,
    pub hbrBackground: HBRUSH,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
    pub hIconSm: HICON,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: WPARAM,
    pub lParam: LPARAM,
    pub time: u32,
    pub pt: POINT,
}

#[repr(C)]
#[derive(Default)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

pub const CS_HREDRAW: u32 = 0x0002;
pub const CS_VREDRAW: u32 = 0x0001;
pub const CS_OWNDC: u32 = 0x0020;
pub const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const SW_SHOWNORMAL: i32 = 1;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_QUIT: u32 = 0x0012;
pub const PM_REMOVE: u32 = 0x0001;
pub const IDC_ARROW: usize = 32512;

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
        menu: HMENU,
        instance: HINSTANCE,
        param: *mut c_void,
    ) -> HWND;
    pub fn ShowWindow(hwnd: HWND, cmd: i32) -> i32;
    pub fn DestroyWindow(hwnd: HWND) -> i32;
    pub fn DefWindowProcW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    pub fn PeekMessageW(
        msg: *mut MSG,
        hwnd: HWND,
        filter_min: u32,
        filter_max: u32,
        remove: u32,
    ) -> i32;
    pub fn TranslateMessage(msg: *const MSG) -> i32;
    pub fn DispatchMessageW(msg: *const MSG) -> LRESULT;
    pub fn PostQuitMessage(exit_code: i32);
    pub fn LoadCursorW(instance: HINSTANCE, name: *const u16) -> HCURSOR;
    pub fn GetClientRect(hwnd: HWND, rect: *mut RECT) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
}

/// Encode a &str as a null-terminated UTF-16 buffer.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
