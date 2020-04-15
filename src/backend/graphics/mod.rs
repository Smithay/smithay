//! Common traits for various ways to renderer on a given graphics backend.
//!
//! Note: Not every API may be supported by every backend

mod cursor;
pub use self::cursor::*;

mod format;
pub use self::format::*;

#[cfg(feature = "renderer_gl")]
pub mod gl;
#[cfg(feature = "renderer_glium")]
pub mod glium;
#[cfg(feature = "renderer_software")]
pub mod software;

/// Error that can happen when swapping buffers.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SwapBuffersError {
    /// The corresponding context has been lost and needs to be recreated.
    ///
    /// All the objects associated to it (textures, buffers, programs, etc.)
    /// need to be recreated from scratch.
    ///
    /// Operations will have no effect. Functions that read textures, buffers, etc.
    /// will return uninitialized data instead.
    #[error("The context has been lost, it needs to be recreated")]
    ContextLost,
    /// The buffers have already been swapped.
    ///
    /// This error can be returned when `swap_buffers` has been called multiple times
    /// without any modification in between.
    #[error("Buffers are already swapped, swap_buffers was called too many times")]
    AlreadySwapped,
    /// Unknown error
    #[error("Unknown error: {0:x}")]
    Unknown(u32),
}
