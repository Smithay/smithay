use wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm};
#[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
use wayland_server::DisplayHandle;

#[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
use crate::backend::{egl::display::EGLBufferReader, renderer::ImportEgl};
use crate::{
    backend::renderer::{ImportDmaWl, ImportMemWl},
    utils::{Buffer, Rectangle},
    wayland::compositor::SurfaceData,
};

use super::{AutoRenderer, AutoRendererError, AutoRendererTexture};

impl ImportMemWl for AutoRenderer {
    fn import_shm_buffer(
        &mut self,
        buffer: &WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<Self::TextureId, Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => ImportMemWl::import_shm_buffer(renderer, buffer, surface, damage)
                .map(AutoRendererTexture::from)
                .map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => {
                ImportMemWl::import_shm_buffer(renderer, buffer, surface, damage)
                    .map(AutoRendererTexture::from)
                    .map_err(AutoRendererError::from)
            }
        }
    }

    fn shm_formats(&self) -> Box<dyn Iterator<Item = wl_shm::Format>> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => ImportMemWl::shm_formats(renderer),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => ImportMemWl::shm_formats(renderer),
        }
    }
}

impl ImportDmaWl for AutoRenderer {
    fn import_dma_buffer(
        &mut self,
        buffer: &wayland_server::protocol::wl_buffer::WlBuffer,
        surface: Option<&crate::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<Self::TextureId, Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => ImportDmaWl::import_dma_buffer(renderer, buffer, surface, damage)
                .map(AutoRendererTexture::from)
                .map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => {
                ImportDmaWl::import_dma_buffer(renderer, buffer, surface, damage)
                    .map(AutoRendererTexture::from)
                    .map_err(AutoRendererError::from)
            }
        }
    }
}

#[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
impl ImportEgl for AutoRenderer {
    fn bind_wl_display(&mut self, display: &DisplayHandle) -> Result<(), crate::backend::egl::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => ImportEgl::bind_wl_display(renderer, display),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => ImportEgl::bind_wl_display(renderer, display),
        }
    }

    fn unbind_wl_display(&mut self) {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => ImportEgl::unbind_wl_display(renderer),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => ImportEgl::unbind_wl_display(renderer),
        }
    }

    fn egl_reader(&self) -> Option<&EGLBufferReader> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => ImportEgl::egl_reader(renderer),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => ImportEgl::egl_reader(renderer),
        }
    }

    fn import_egl_buffer(
        &mut self,
        buffer: &WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<Self::TextureId, Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => ImportEgl::import_egl_buffer(renderer, buffer, surface, damage)
                .map(AutoRendererTexture::from)
                .map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => ImportEgl::import_egl_buffer(renderer, buffer, surface, damage)
                .map(AutoRendererTexture::from)
                .map_err(AutoRendererError::from),
        }
    }
}
