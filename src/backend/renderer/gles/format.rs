//! GL color format conversion helpers

use super::ffi::{self, types::GLenum};
use crate::backend::allocator::{
    format::{get_transparent, has_alpha},
    Fourcc,
};

/// Returns (internal_format, read_format, type)
pub const fn fourcc_to_gl_formats(value: Fourcc) -> Option<(GLenum, GLenum, GLenum)> {
    let Some(value) = (if has_alpha(value) {
        Some(value)
    } else {
        get_transparent(value)
    }) else {
        return None; // ? not allowed in const fn
    };

    match value {
        Fourcc::Abgr8888 => Some((ffi::RGBA8, ffi::RGBA, ffi::UNSIGNED_BYTE)),
        Fourcc::Argb8888 => Some((ffi::BGRA_EXT, ffi::BGRA_EXT, ffi::UNSIGNED_BYTE)),
        Fourcc::Abgr2101010 => Some((ffi::RGB10_A2, ffi::RGBA, ffi::UNSIGNED_INT_2_10_10_10_REV)),
        Fourcc::Abgr16161616f => Some((ffi::RGBA16F, ffi::RGBA, ffi::HALF_FLOAT)),
        _ => None,
    }
}

/// Returns the fourcc for a given internal format
pub const fn gl_internal_format_to_fourcc(format: GLenum) -> Option<Fourcc> {
    match format {
        ffi::RGBA | ffi::RGBA8 => Some(Fourcc::Abgr8888),
        ffi::BGRA_EXT => Some(Fourcc::Argb8888),
        ffi::RGB8 => Some(Fourcc::Bgr888),
        ffi::RGB10_A2 => Some(Fourcc::Abgr2101010),
        ffi::RGBA16F => Some(Fourcc::Abgr16161616f),
        _ => None,
    }
}

/// Returns the fourcc for a given read format and type
pub const fn gl_read_format_to_fourcc(format: GLenum, type_: GLenum) -> Option<Fourcc> {
    match (format, type_) {
        (ffi::RGBA, ffi::UNSIGNED_BYTE) => Some(Fourcc::Abgr8888),
        (ffi::BGRA_EXT, ffi::UNSIGNED_BYTE) => Some(Fourcc::Argb8888),
        (ffi::RGB, ffi::UNSIGNED_BYTE) => Some(Fourcc::Bgr888),
        (ffi::RGBA, ffi::UNSIGNED_INT_2_10_10_10_REV) => Some(Fourcc::Abgr2101010),
        (ffi::RGBA, ffi::HALF_FLOAT) => Some(Fourcc::Abgr16161616f),
        _ => None,
    }
}

/// Returns a recommended read format and type for a given internal format
pub const fn gl_read_for_internal(format: GLenum) -> Option<(GLenum, GLenum)> {
    match format {
        ffi::RGBA | ffi::RGBA8 => Some((ffi::RGBA, ffi::UNSIGNED_BYTE)),
        ffi::BGRA_EXT => Some((ffi::BGRA_EXT, ffi::UNSIGNED_BYTE)),
        ffi::RGB8 => Some((ffi::RGB, ffi::UNSIGNED_BYTE)),
        ffi::RGB10_A2 => Some((ffi::RGBA, ffi::UNSIGNED_INT_2_10_10_10_REV)),
        ffi::RGBA16F => Some((ffi::RGBA, ffi::HALF_FLOAT)),
        _ => None,
    }
}

/// Returns the bits per pixel for a given read format and type
pub const fn gl_bpp(format: GLenum, type_: GLenum) -> Option<usize> {
    match (format, type_) {
        (ffi::RGB, ffi::UNSIGNED_BYTE) => Some(24),
        (ffi::RGBA, ffi::UNSIGNED_BYTE)
        | (ffi::BGRA_EXT, ffi::UNSIGNED_BYTE)
        | (ffi::RGBA, ffi::UNSIGNED_INT_2_10_10_10_REV) => Some(32),
        (ffi::RGBA, ffi::HALF_FLOAT) => Some(64),
        _ => None,
    }
}
