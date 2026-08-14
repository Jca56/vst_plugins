//! Minimal byte-view helpers for GPU uploads — our stand-in for bytemuck.
//!
//! Only implement `Pod` for `#[repr(C)]` types made purely of plain
//! floats/ints with no padding: every byte must be initialized for the raw
//! view to be sound.

pub(crate) unsafe trait Pod: Copy + 'static {}

pub(crate) fn bytes_of<T: Pod>(value: &T) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
    }
}

pub(crate) fn cast_slice<T: Pod>(values: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values))
    }
}
