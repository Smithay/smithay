use crate::{
    backend::renderer::{Blit, BlitFrame, TextureFilter},
    utils::{Physical, Rectangle},
};

#[cfg(feature = "renderer_gl")]
use crate::backend::renderer::gles::GlesTarget;

use super::{AutoRenderer, AutoRendererError, AutoRendererFrame};

impl Blit for AutoRenderer {
    fn blit(
        &mut self,
        from: &Self::Framebuffer<'_>,
        to: &mut Self::Framebuffer<'_>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: crate::backend::renderer::TextureFilter,
    ) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => {
                Blit::blit(renderer, from.try_into()?, to.try_into()?, src, dst, filter)
                    .map_err(AutoRendererError::from)
            }
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(_) => Err(AutoRendererError::Unsupported),
        }
    }
}

#[cfg(feature = "renderer_gl")]
impl<'buffer> BlitFrame<GlesTarget<'buffer>> for AutoRendererFrame<'_, 'buffer> {
    fn blit_to(
        &mut self,
        to: &mut GlesTarget<'buffer>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererFrame::Gles(renderer) => {
                BlitFrame::blit_to(renderer, to, src, dst, filter).map_err(AutoRendererError::from)
            }
            #[cfg(feature = "renderer_pixman")]
            AutoRendererFrame::Pixman(_) => Err(AutoRendererError::Unsupported),
        }
    }

    fn blit_from(
        &mut self,
        from: &GlesTarget<'buffer>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererFrame::Gles(renderer) => {
                BlitFrame::blit_from(renderer, from, src, dst, filter).map_err(AutoRendererError::from)
            }
            #[cfg(feature = "renderer_pixman")]
            AutoRendererFrame::Pixman(_) => Err(AutoRendererError::Unsupported),
        }
    }
}
