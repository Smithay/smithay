//! Common traits for various ways to renderer on a given graphics backend.
//!
//! Note: Not every API may be supported by every backend

mod errors;
pub use self::errors::*;

mod cursor;
pub use self::cursor::*;

#[cfg(feature = "renderer_gl")]
pub mod gl;
#[cfg(feature = "renderer_glium")]
pub mod glium;
#[cfg(feature = "renderer_software")]
pub mod software;
