use crate::backend::{
    allocator::{Buffer, Slot},
    renderer::Renderer,
};

#[cfg(all(feature = "backend_drm", feature = "renderer_pixman"))]
mod dumb;
#[cfg(feature = "backend_gbm")]
mod gbm;

#[cfg(all(feature = "backend_drm", feature = "renderer_pixman"))]
pub use dumb::{DumbBufferError, DumbBufferRenderTarget};

/// Trait for getting a bindable render target for an allocator buffer
pub trait AsRenderTarget<B: Buffer, R: Renderer> {
    /// Bindable render target
    type Target<'buffer>
    where
        B: 'buffer;

    /// Error type
    type Error: std::error::Error + Send + Sync + 'static;

    /// Get bindable render target for provided allocator buffer
    fn as_render_target<'buffer>(
        renderer: &mut R,
        slot: &'buffer mut Slot<B>,
    ) -> Result<Self::Target<'buffer>, Self::Error>;
}

/// Default implementation of [`AsRenderTarget`]
#[derive(Debug)]
pub struct DefaultAsRenderTarget;
