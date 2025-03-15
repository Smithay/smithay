use crate::backend::{
    allocator::{Buffer, Slot},
    renderer::{pixman::PixmanRenderer, Bind},
};

use super::{AsRenderTarget, DefaultAsRenderTarget};

/// Wrapper for binding a dumb buffer
pub struct DumbBufferRenderTarget<'buffer> {
    /// SAFETY: The image borrows data from the mapping, so
    /// we need to be careful to always destroy it first
    image: pixman::Image<'buffer, 'static>,
    _mapping: drm::control::dumbbuffer::DumbMapping<'buffer>,
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
    ) -> Result<
        crate::backend::renderer::pixman::PixmanTarget<'a>,
        crate::backend::renderer::pixman::PixmanError,
    > {
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

impl
    AsRenderTarget<
        crate::backend::allocator::dumb::DumbBuffer,
        crate::backend::renderer::pixman::PixmanRenderer,
    > for DefaultAsRenderTarget
{
    type Target<'buffer> = DumbBufferRenderTarget<'buffer>;
    type Error = DumbBufferError;

    fn as_render_target<'buffer>(
        _renderer: &mut crate::backend::renderer::pixman::PixmanRenderer,
        slot: &'buffer mut Slot<crate::backend::allocator::dumb::DumbBuffer>,
    ) -> Result<Self::Target<'buffer>, Self::Error> {
        use drm::buffer::Buffer;
        let width = slot.width();
        let height = slot.height();
        let stride = slot.handle().pitch();
        let format = pixman::FormatCode::try_from(slot.format().code)?;
        let mut mapping = slot.map().unwrap();
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

        Ok(DumbBufferRenderTarget {
            image,
            _mapping: mapping,
        })
    }
}
