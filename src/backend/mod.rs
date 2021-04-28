//! Backend (rendering/input) creation helpers
//!
//! Collection of common traits and implementation about
//! rendering onto various targets and receiving input
//! from various sources.
//!
//! Supported graphics backends:
//!
//! - winit
//! - drm
//!
//! Supported input backends:
//!
//! - winit
//! - libinput

// TODO TEMPORARY
#![allow(missing_docs)]

//pub mod graphics;
pub mod allocator;
pub mod input;
pub mod renderer;

#[cfg(feature = "backend_drm")]
pub mod drm;
#[cfg(feature = "backend_egl")]
pub mod egl;
#[cfg(feature = "backend_libinput")]
pub mod libinput;
#[cfg(feature = "backend_session")]
pub mod session;
#[cfg(feature = "backend_udev")]
pub mod udev;

#[cfg(feature = "backend_winit")]
pub mod winit;

/// Error that can happen when swapping buffers.
#[derive(Debug, thiserror::Error)]
pub enum SwapBuffersError {
    /// The buffers have already been swapped.
    ///
    /// This error can be returned when `swap_buffers` has been called multiple times
    /// without any modification in between.
    #[error("Buffers are already swapped, swap_buffers was called too many times")]
    AlreadySwapped,
    /// The corresponding context has been lost and needs to be recreated.
    ///
    /// All the objects associated to it (textures, buffers, programs, etc.)
    /// need to be recreated from scratch. Underlying resources like native surfaces
    /// might also need to be recreated.
    ///
    /// Operations will have no effect. Functions that read textures, buffers, etc.
    /// will return uninitialized data instead.
    #[error("The context has been lost, it needs to be recreated: {0}")]
    ContextLost(Box<dyn std::error::Error>),
    /// A temporary condition caused to rendering to fail.
    ///
    /// Depending on the underlying error this *might* require fixing internal state of the rendering backend,
    /// but failures mapped to `TemporaryFailure` are always recoverable without re-creating the entire stack,
    /// as is represented by `ContextLost`.
    ///
    /// Proceed after investigating the source to reschedule another full rendering step or just this page_flip at a later time.
    /// If the root cause cannot be discovered and subsequent renderings also fail, it is advised to fallback to
    /// recreation.
    #[error("A temporary condition caused the page flip to fail: {0}")]
    TemporaryFailure(Box<dyn std::error::Error>),
}
