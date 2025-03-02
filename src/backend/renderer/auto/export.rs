use crate::{
    backend::renderer::ExportMem,
    utils::{Buffer, Rectangle},
};

use super::{AutoRenderer, AutoRendererError, AutoRendererMapping};

impl ExportMem for AutoRenderer {
    type TextureMapping = AutoRendererMapping;

    fn copy_framebuffer(
        &mut self,
        target: &Self::Framebuffer<'_>,
        region: Rectangle<i32, Buffer>,
        format: gbm::Format,
    ) -> Result<Self::TextureMapping, Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => {
                ExportMem::copy_framebuffer(renderer, target.try_into()?, region, format)
                    .map(AutoRendererMapping::from)
                    .map_err(AutoRendererError::from)
            }
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => {
                ExportMem::copy_framebuffer(renderer, target.try_into()?, region, format)
                    .map(AutoRendererMapping::from)
                    .map_err(AutoRendererError::from)
            }
        }
    }

    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, Buffer>,
        format: gbm::Format,
    ) -> Result<Self::TextureMapping, Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => {
                ExportMem::copy_texture(renderer, texture.try_into()?, region, format)
                    .map(AutoRendererMapping::from)
                    .map_err(AutoRendererError::from)
            }
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => {
                ExportMem::copy_texture(renderer, texture.try_into()?, region, format)
                    .map(AutoRendererMapping::from)
                    .map_err(AutoRendererError::from)
            }
        }
    }

    fn can_read_texture(&mut self, texture: &Self::TextureId) -> Result<bool, Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => {
                ExportMem::can_read_texture(renderer, texture.try_into()?).map_err(AutoRendererError::from)
            }
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => {
                ExportMem::can_read_texture(renderer, texture.try_into()?).map_err(AutoRendererError::from)
            }
        }
    }

    fn map_texture<'a>(
        &mut self,
        texture_mapping: &'a Self::TextureMapping,
    ) -> Result<&'a [u8], Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => {
                ExportMem::map_texture(renderer, texture_mapping.try_into()?).map_err(AutoRendererError::from)
            }
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => {
                ExportMem::map_texture(renderer, texture_mapping.try_into()?).map_err(AutoRendererError::from)
            }
        }
    }
}
