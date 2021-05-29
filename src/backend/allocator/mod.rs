//! Buffer allocation and management.
//!
//! Collection of common traits and implementations around
//! buffer creation and handling from various sources.
//!
//! Allocators provided:
//! - Dumb Buffers through [`crate::backend::drm::DrmDevice`]
//! - Gbm Buffers through [`::gbm::Device`]
//!
//! Buffer types supported:
//! - [DumbBuffers](dumb::DumbBuffer)
//! - [GbmBuffers](::gbm::BufferObject)
//! - [DmaBufs](dmabuf::Dmabuf)
//!
//! Helpers:
//! - [`Swapchain`] to help with buffer management for framebuffers

pub mod dmabuf;
#[cfg(feature = "backend_drm")]
pub mod dumb;
#[cfg(feature = "backend_gbm")]
pub mod gbm;

mod swapchain;
pub use swapchain::{Slot, Swapchain};

pub use drm_fourcc::{
    DrmFormat as Format, DrmFourcc as Fourcc, DrmModifier as Modifier, DrmVendor as Vendor,
    UnrecognizedFourcc, UnrecognizedVendor,
};

/// Common trait describing common properties of most types of buffers.
pub trait Buffer {
    /// Width of the two-dimensional buffer
    fn width(&self) -> u32;
    /// Height of the two-dimensional buffer
    fn height(&self) -> u32;
    /// Size (w x h) of the two-dimensional buffer
    fn size(&self) -> (u32, u32) {
        (self.width(), self.height())
    }
    /// Pixel format of the buffer
    fn format(&self) -> Format;
}

/// Interface to create Buffers
pub trait Allocator<B: Buffer> {
    /// Error type thrown if allocations fail
    type Error: std::error::Error;

    /// Try to create a buffer with the given dimensions and pixel format
    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: &[Modifier],
    ) -> Result<B, Self::Error>;
}
