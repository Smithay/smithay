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
pub mod format;
#[cfg(feature = "backend_gbm")]
pub mod gbm;
#[cfg(feature = "backend_vulkan")]
pub mod vulkan;

mod swapchain;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use crate::utils::{Buffer as BufferCoords, Size};
pub use swapchain::{Slot, Swapchain};

pub use drm_fourcc::{
    DrmFormat as Format, DrmFourcc as Fourcc, DrmModifier as Modifier, DrmVendor as Vendor,
    UnrecognizedFourcc, UnrecognizedVendor,
};

/// Common trait describing common properties of most types of buffers.
pub trait Buffer {
    /// Width of the two-dimensional buffer
    fn width(&self) -> u32 {
        self.size().w as u32
    }
    /// Height of the two-dimensional buffer
    fn height(&self) -> u32 {
        self.size().h as u32
    }
    /// Size of the two-dimensional buffer
    fn size(&self) -> Size<i32, BufferCoords>;
    /// Pixel format of the buffer
    fn format(&self) -> Format;
}

/// Interface to create Buffers
pub trait Allocator {
    /// Buffer type produced by this allocator
    type Buffer: Buffer;
    /// Error type thrown if allocations fail
    type Error: std::error::Error;

    /// Try to create a buffer with the given dimensions and pixel format
    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: &[Modifier],
    ) -> Result<Self::Buffer, Self::Error>;
}

// General implementations for interior mutability.

impl<A: Allocator> Allocator for Arc<Mutex<A>> {
    type Buffer = A::Buffer;
    type Error = A::Error;

    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: &[Modifier],
    ) -> Result<Self::Buffer, Self::Error> {
        let mut guard = self.lock().unwrap();
        guard.create_buffer(width, height, fourcc, modifiers)
    }
}

impl<A: Allocator> Allocator for Rc<RefCell<A>> {
    type Buffer = A::Buffer;
    type Error = A::Error;

    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: &[Modifier],
    ) -> Result<Self::Buffer, Self::Error> {
        self.borrow_mut().create_buffer(width, height, fourcc, modifiers)
    }
}

impl<B: Buffer, E: std::error::Error> Allocator for Box<dyn Allocator<Buffer = B, Error = E> + 'static> {
    type Buffer = B;
    type Error = E;

    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: &[Modifier],
    ) -> Result<B, E> {
        (**self).create_buffer(width, height, fourcc, modifiers)
    }
}
