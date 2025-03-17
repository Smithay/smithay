use drm_fourcc::DrmFormat;

use crate::{
    backend::{
        allocator::format::FormatSet,
        renderer::{ImportDma, ImportMem},
    },
    utils::{Buffer, Rectangle},
};

use super::{AutoRenderer, AutoRendererError, AutoRendererTexture};

impl ImportMem for AutoRenderer {
    fn import_memory(
        &mut self,
        data: &[u8],
        format: gbm::Format,
        size: crate::utils::Size<i32, Buffer>,
        flipped: bool,
    ) -> Result<Self::TextureId, Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => ImportMem::import_memory(renderer, data, format, size, flipped)
                .map(AutoRendererTexture::from)
                .map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => ImportMem::import_memory(renderer, data, format, size, flipped)
                .map(AutoRendererTexture::from)
                .map_err(AutoRendererError::from),
        }
    }

    fn update_memory(
        &mut self,
        texture: &Self::TextureId,
        data: &[u8],
        region: Rectangle<i32, Buffer>,
    ) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => {
                ImportMem::update_memory(renderer, texture.try_into()?, data, region)
                    .map_err(AutoRendererError::from)
            }
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => {
                ImportMem::update_memory(renderer, texture.try_into()?, data, region)
                    .map_err(AutoRendererError::from)
            }
        }
    }

    fn mem_formats(&self) -> Box<dyn Iterator<Item = gbm::Format>> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => ImportMem::mem_formats(renderer),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => ImportMem::mem_formats(renderer),
        }
    }
}

impl ImportDma for AutoRenderer {
    fn dmabuf_formats(&self) -> FormatSet {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => ImportDma::dmabuf_formats(renderer),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => ImportDma::dmabuf_formats(renderer),
        }
    }

    fn has_dmabuf_format(&self, format: DrmFormat) -> bool {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => ImportDma::has_dmabuf_format(renderer, format),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => ImportDma::has_dmabuf_format(renderer, format),
        }
    }

    fn import_dmabuf(
        &mut self,
        dmabuf: &crate::backend::allocator::dmabuf::Dmabuf,
        damage: Option<&[Rectangle<i32, Buffer>]>,
    ) -> Result<Self::TextureId, Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => ImportDma::import_dmabuf(renderer, dmabuf, damage)
                .map(AutoRendererTexture::from)
                .map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => ImportDma::import_dmabuf(renderer, dmabuf, damage)
                .map(AutoRendererTexture::from)
                .map_err(AutoRendererError::from),
        }
    }
}
