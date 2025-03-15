use crate::backend::{
    allocator::{dumb::DumbBuffer, Buffer, Slot},
    renderer::{
        pixman::{PixmanError, PixmanRenderer, PixmanTarget},
        Bind,
    },
};

use super::{AsRenderTarget, DefaultAsRenderTarget};

/// Wrapper for binding a dumb buffer
pub struct DumbBufferRenderTarget<'buffer> {
    /// SAFETY: The image borrows data from the mapping, so
    /// we need to be careful to always destroy it first
    image: pixman::Image<'buffer, 'static>,
    _mapping: drm::control::dumbbuffer::DumbMapping<'buffer>,
}

impl<'buffer> DumbBufferRenderTarget<'buffer> {
    /// Create a [`DumbBufferRenderTarget`] from a [`DumbBuffer`]
    pub fn from_dumb_buffer(buffer: &'buffer mut DumbBuffer) -> Result<Self, DumbBufferError> {
        use drm::buffer::Buffer;
        let width = buffer.width();
        let height = buffer.height();
        let stride = buffer.handle().pitch();
        let format = pixman::FormatCode::try_from(buffer.format().code)?;
        let mut mapping = buffer.map().unwrap();
        let image = unsafe {
            pixman::Image::from_raw_mut(
                format,
                width as usize,
                height as usize,
                mapping.as_mut_ptr() as *mut _,
                stride as usize,
                false,
            )
        }?;

        Ok(Self {
            image,
            _mapping: mapping,
        })
    }
}

impl std::fmt::Debug for DumbBufferRenderTarget<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DumbBufferRenderTarget")
            .field("image", &self.image)
            .finish()
    }
}

impl<'buffer> Bind<DumbBufferRenderTarget<'buffer>> for PixmanRenderer {
    fn bind<'a>(
        &mut self,
        target: &'a mut DumbBufferRenderTarget<'buffer>,
    ) -> Result<PixmanTarget<'a>, PixmanError> {
        self.bind(&mut target.image)
    }
}

/// Error for dumb buffer as render target
#[derive(Debug, thiserror::Error)]
pub enum DumbBufferError {
    /// Unsupported drm fourcc
    #[error(transparent)]
    UnsupportedDrmFourcc(#[from] pixman::UnsupportedDrmFourcc),
    /// Image creation failed
    #[error(transparent)]
    CreateFailed(#[from] pixman::CreateFailed),
}

impl AsRenderTarget<DumbBuffer, PixmanRenderer> for DefaultAsRenderTarget {
    type Target<'buffer> = DumbBufferRenderTarget<'buffer>;
    type Error = DumbBufferError;

    fn as_render_target<'buffer>(
        _renderer: &mut PixmanRenderer,
        slot: &'buffer mut Slot<DumbBuffer>,
    ) -> Result<Self::Target<'buffer>, Self::Error> {
        DumbBufferRenderTarget::from_dumb_buffer(slot)
    }
}
