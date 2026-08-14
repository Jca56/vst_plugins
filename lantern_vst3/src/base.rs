//! Core types shared by every VST3 interface: result codes, interface IDs,
//! and the string helpers for the fixed-size char fields in the ABI structs.
//!
//! Windows-only by design (Ableton-under-Wine is a Windows host): tresult
//! values are the COM error codes and TUIDs use COM GUID byte order.

#![allow(non_upper_case_globals)]

/// COM-style result code (i32; negative = error).
pub type tresult = i32;

pub const kNoInterface: tresult = -2_147_467_262; // 0x80004002 E_NOINTERFACE
pub const kResultOk: tresult = 0;
pub const kResultTrue: tresult = 0;
pub const kResultFalse: tresult = 1;
pub const kInvalidArgument: tresult = -2_147_024_809; // 0x80070057 E_INVALIDARG
pub const kNotImplemented: tresult = -2_147_467_263; // 0x80004001 E_NOTIMPL
pub const kInternalError: tresult = -2_147_467_259; // 0x80004005 E_FAIL
pub const kNotInitialized: tresult = -2_147_418_113; // 0x8000FFFF E_UNEXPECTED
pub const kOutOfMemory: tresult = -2_147_024_882; // 0x8007000E E_OUTOFMEMORY

/// 16-byte interface/class ID.
pub type TUID = [u8; 16];

/// 8-bit bool as used across the ABI.
pub type TBool = u8;

/// UTF-16 code unit (the ABI uses i16).
pub type TChar = i16;

/// Fixed 128-unit UTF-16 string field.
pub type String128 = [TChar; 128];

/// Build a TUID from canonical GUID parts ("D1D1D1D1-D2D2-D3D3-B0B1-B2B3B4B5B6B7")
/// in Windows/COM byte order: data1/2/3 little-endian, trailing 8 bytes as
/// written. This matches what Windows hosts pass to queryInterface.
pub const fn iid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> TUID {
    [
        (d1 & 0xFF) as u8,
        ((d1 >> 8) & 0xFF) as u8,
        ((d1 >> 16) & 0xFF) as u8,
        (d1 >> 24) as u8,
        (d2 & 0xFF) as u8,
        (d2 >> 8) as u8,
        (d3 & 0xFF) as u8,
        (d3 >> 8) as u8,
        d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7],
    ]
}

pub fn tuid_eq(a: *const TUID, b: &TUID) -> bool {
    if a.is_null() {
        return false;
    }
    unsafe { &*a == b }
}

/// Copy a &str into a fixed-size C char (i8) field, NUL-terminated, truncating.
pub fn write_char8(dst: &mut [i8], src: &str) {
    let mut i = 0;
    for byte in src.bytes() {
        if i >= dst.len() - 1 {
            break;
        }
        dst[i] = byte as i8;
        i += 1;
    }
    dst[i] = 0;
}

/// Copy a &str into a fixed-size UTF-16 (i16) field, NUL-terminated, truncating.
pub fn write_char16(dst: &mut [i16], src: &str) {
    let mut i = 0;
    for unit in src.encode_utf16() {
        if i >= dst.len() - 1 {
            break;
        }
        dst[i] = unit as i16;
        i += 1;
    }
    dst[i] = 0;
}

/// Read a NUL-terminated UTF-16 (i16) buffer into a String (lossy).
///
/// # Safety
/// `src` must point to a NUL-terminated buffer of at most `max` units.
pub unsafe fn read_char16(src: *const i16, max: usize) -> String {
    if src.is_null() {
        return String::new();
    }
    let mut units = Vec::new();
    for i in 0..max {
        let unit = *src.add(i);
        if unit == 0 {
            break;
        }
        units.push(unit as u16);
    }
    String::from_utf16_lossy(&units)
}
