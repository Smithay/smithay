//! Trait and data structures to describe types, that can be exported to a drm framebuffer

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::wl_buffer::WlBuffer;

use crate::backend::{allocator::Buffer, renderer::element::UnderlyingStorage};

use super::{DrmDeviceFd, Framebuffer};

#[cfg(feature = "backend_drm")]
pub mod dumb;
#[cfg(feature = "backend_gbm")]
pub mod gbm;

/// Possible buffers to export as a framebuffer using [`ExportFramebuffer`]
#[derive(Debug)]
pub enum ExportBuffer<'a, B: Buffer> {
    /// A wayland buffer
    #[cfg(feature = "wayland_frontend")]
    Wayland(&'a WlBuffer),
    /// A [`Allocator`][crate::backend::allocator::Allocator] buffer
    Allocator(&'a B),
}

impl<'a, B: Buffer> ExportBuffer<'a, B> {
    /// Create the export buffer from an [`UnderlyingStorage`]
    #[inline]
    pub fn from_underlying_storage(storage: &'a UnderlyingStorage<'_>) -> Option<Self> {
        match storage {
            #[cfg(feature = "wayland_frontend")]
            UnderlyingStorage::Wayland(buffer) => Some(Self::Wayland(buffer)),
            UnderlyingStorage::Memory { .. } => None,
        }
    }
}

/// Export a [`ExportBuffer`] as a framebuffer
pub trait ExportFramebuffer<B: Buffer>
where
    B: Buffer,
{
    /// Type of the framebuffer
    type Framebuffer: Framebuffer;

    /// Type of the error
    type Error: std::error::Error;

    /// Add a framebuffer for the specified buffer
    fn add_framebuffer(
        &self,
        drm: &DrmDeviceFd,
        buffer: ExportBuffer<'_, B>,
        use_opaque: bool,
    ) -> Result<Option<Self::Framebuffer>, Self::Error>;

    /// Test if the provided buffer is eligible for adding a framebuffer
    fn can_add_framebuffer(&self, buffer: &ExportBuffer<'_, B>) -> bool;
}

impl<F, B> ExportFramebuffer<B> for Arc<Mutex<F>>
where
    F: ExportFramebuffer<B>,
    B: Buffer,
{
    type Framebuffer = <F as ExportFramebuffer<B>>::Framebuffer;
    type Error = <F as ExportFramebuffer<B>>::Error;

    #[inline]
    fn add_framebuffer(
        &self,
        drm: &DrmDeviceFd,
        buffer: ExportBuffer<'_, B>,
        use_opaque: bool,
    ) -> Result<Option<Self::Framebuffer>, Self::Error> {
        let guard = self.lock().unwrap();
        guard.add_framebuffer(drm, buffer, use_opaque)
    }

    #[inline]
    fn can_add_framebuffer(&self, buffer: &ExportBuffer<'_, B>) -> bool {
        let guard = self.lock().unwrap();
        guard.can_add_framebuffer(buffer)
    }
}

impl<F, B> ExportFramebuffer<B> for Rc<RefCell<F>>
where
    F: ExportFramebuffer<B>,
    B: Buffer,
{
    type Framebuffer = <F as ExportFramebuffer<B>>::Framebuffer;
    type Error = <F as ExportFramebuffer<B>>::Error;

    #[inline]
    fn add_framebuffer(
        &self,
        drm: &DrmDeviceFd,
        buffer: ExportBuffer<'_, B>,
        use_opaque: bool,
    ) -> Result<Option<Self::Framebuffer>, Self::Error> {
        self.borrow().add_framebuffer(drm, buffer, use_opaque)
    }

    #[inline]
    fn can_add_framebuffer(&self, buffer: &ExportBuffer<'_, B>) -> bool {
        self.borrow().can_add_framebuffer(buffer)
    }
}
