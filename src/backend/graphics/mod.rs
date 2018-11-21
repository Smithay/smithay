//! Common traits for various ways to renderer on a given graphics backend.
//!
//! Note: Not every API may be supported by every backend


mod cursor;
pub use self::cursor::*;

#[cfg(feature = "renderer_gl")]
pub mod gl;
#[cfg(feature = "renderer_glium")]
pub mod glium;
pub mod software;
