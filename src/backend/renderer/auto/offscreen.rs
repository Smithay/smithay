use drm_fourcc::DrmFourcc;

use crate::{
    backend::renderer::Offscreen,
    utils::{Buffer, Size},
};

#[cfg(feature = "renderer_gl")]
use crate::backend::renderer::gles::{GlesRenderbuffer, GlesTexture};

#[cfg(feature = "renderer_pixman")]
use pixman::Image;

use super::{AutoRenderer, AutoRendererError};

#[cfg(feature = "renderer_gl")]
impl Offscreen<GlesTexture> for AutoRenderer {
    fn create_buffer(
        &mut self,
        format: DrmFourcc,
        size: Size<i32, Buffer>,
    ) -> Result<GlesTexture, Self::Error> {
        let AutoRenderer::Gles(renderer) = self else {
            return Err(AutoRendererError::Unsupported);
        };
        Offscreen::<GlesTexture>::create_buffer(renderer, format, size).map_err(AutoRendererError::from)
    }
}

#[cfg(feature = "renderer_gl")]
impl Offscreen<GlesRenderbuffer> for AutoRenderer {
    fn create_buffer(
        &mut self,
        format: DrmFourcc,
        size: Size<i32, Buffer>,
    ) -> Result<GlesRenderbuffer, Self::Error> {
        let AutoRenderer::Gles(renderer) = self else {
            return Err(AutoRendererError::Unsupported);
        };
        Offscreen::<GlesRenderbuffer>::create_buffer(renderer, format, size).map_err(AutoRendererError::from)
    }
}

#[cfg(feature = "renderer_pixman")]
impl Offscreen<Image<'static, 'static>> for AutoRenderer {
    fn create_buffer(
        &mut self,
        format: DrmFourcc,
        size: Size<i32, Buffer>,
    ) -> Result<Image<'static, 'static>, Self::Error> {
        let AutoRenderer::Pixman(renderer) = self else {
            return Err(AutoRendererError::Unsupported);
        };
        Offscreen::<Image<'static, 'static>>::create_buffer(renderer, format, size)
            .map_err(AutoRendererError::from)
    }
}
