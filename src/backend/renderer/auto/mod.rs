//! Unified renderer
use crate::{
    backend::renderer::sync::SyncPoint,
    utils::{Physical, Size, Transform},
};

#[cfg(feature = "renderer_gl")]
use crate::backend::renderer::gles::GlesRenderer;

#[cfg(feature = "renderer_pixman")]
use crate::backend::renderer::pixman::PixmanRenderer;

mod error;
mod frame;
mod mapping;
mod target;
mod texture;

mod bind;
mod blit;
mod export;
mod import;
#[cfg(feature = "wayland_frontend")]
mod import_wl;
mod offscreen;

pub use error::AutoRendererError;
pub use frame::AutoRendererFrame;
pub use mapping::AutoRendererMapping;
pub use target::AutoRendererTarget;
pub use texture::AutoRendererTexture;

use super::{DebugFlags, Renderer, RendererSuper, TextureFilter};

/// Auto-renderer
#[derive(Debug)]
pub enum AutoRenderer {
    /// Gles renderer
    #[cfg(feature = "renderer_gl")]
    Gles(GlesRenderer),
    /// Pixman renderer
    #[cfg(feature = "renderer_pixman")]
    Pixman(PixmanRenderer),
}

#[cfg(feature = "renderer_gl")]
impl From<GlesRenderer> for AutoRenderer {
    fn from(value: GlesRenderer) -> Self {
        Self::Gles(value)
    }
}

#[cfg(feature = "renderer_pixman")]
impl From<PixmanRenderer> for AutoRenderer {
    fn from(value: PixmanRenderer) -> Self {
        Self::Pixman(value)
    }
}

impl RendererSuper for AutoRenderer {
    type Error = AutoRendererError;

    type TextureId = AutoRendererTexture;

    type Framebuffer<'buffer> = AutoRendererTarget<'buffer>;

    type Frame<'frame, 'buffer>
        = AutoRendererFrame<'frame, 'buffer>
    where
        'buffer: 'frame,
        Self: 'frame;
}

impl Renderer for AutoRenderer {
    fn id(&self) -> usize {
        todo!()
    }

    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => {
                renderer.downscale_filter(filter).map_err(AutoRendererError::from)
            }
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => {
                renderer.downscale_filter(filter).map_err(AutoRendererError::from)
            }
        }
    }

    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => renderer.upscale_filter(filter).map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => {
                renderer.upscale_filter(filter).map_err(AutoRendererError::from)
            }
        }
    }

    fn set_debug_flags(&mut self, flags: DebugFlags) {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => renderer.set_debug_flags(flags),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => renderer.set_debug_flags(flags),
        }
    }

    fn debug_flags(&self) -> DebugFlags {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => renderer.debug_flags(),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => renderer.debug_flags(),
        }
    }

    fn render<'frame, 'buffer>(
        &'frame mut self,
        framebuffer: &'frame mut Self::Framebuffer<'buffer>,
        output_size: Size<i32, Physical>,
        dst_transform: Transform,
    ) -> Result<Self::Frame<'frame, 'buffer>, Self::Error>
    where
        'buffer: 'frame,
    {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => renderer
                .render(framebuffer.try_into()?, output_size, dst_transform)
                .map(AutoRendererFrame::from)
                .map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => renderer
                .render(framebuffer.try_into()?, output_size, dst_transform)
                .map(AutoRendererFrame::from)
                .map_err(AutoRendererError::from),
        }
    }

    fn wait(&mut self, sync: &SyncPoint) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => renderer.wait(sync).map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => renderer.wait(sync).map_err(AutoRendererError::from),
        }
    }

    fn cleanup_texture_cache(&mut self) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => renderer.cleanup_texture_cache().map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => {
                renderer.cleanup_texture_cache().map_err(AutoRendererError::from)
            }
        }
    }
}
