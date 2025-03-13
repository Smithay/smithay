use crate::backend::{
    allocator::{dmabuf::Dmabuf, format::FormatSet},
    renderer::Bind,
};

#[cfg(feature = "renderer_gl")]
use crate::backend::{
    egl::EGLSurface,
    renderer::gles::{GlesRenderbuffer, GlesTexture},
};

#[cfg(feature = "renderer_pixman")]
use pixman::Image;

use super::{AutoRenderer, AutoRendererError, AutoRendererTarget};

impl Bind<Dmabuf> for AutoRenderer {
    fn bind<'a>(&mut self, target: &'a mut Dmabuf) -> Result<Self::Framebuffer<'a>, Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => Bind::<Dmabuf>::bind(renderer, target)
                .map(AutoRendererTarget::from)
                .map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => Bind::<Dmabuf>::bind(renderer, target)
                .map(AutoRendererTarget::from)
                .map_err(AutoRendererError::from),
        }
    }

    fn supported_formats(&self) -> Option<FormatSet> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRenderer::Gles(renderer) => Bind::<Dmabuf>::supported_formats(renderer),
            #[cfg(feature = "renderer_pixman")]
            AutoRenderer::Pixman(renderer) => Bind::<Dmabuf>::supported_formats(renderer),
        }
    }
}

#[cfg(feature = "renderer_gl")]
impl Bind<EGLSurface> for AutoRenderer {
    fn bind<'a>(&mut self, target: &'a mut EGLSurface) -> Result<Self::Framebuffer<'a>, Self::Error> {
        let AutoRenderer::Gles(renderer) = self else {
            return Err(AutoRendererError::Unsupported);
        };
        Bind::<EGLSurface>::bind(renderer, target)
            .map(AutoRendererTarget::from)
            .map_err(AutoRendererError::from)
    }

    fn supported_formats(&self) -> Option<FormatSet> {
        let AutoRenderer::Gles(renderer) = self else {
            return None;
        };
        Bind::<Dmabuf>::supported_formats(renderer)
    }
}

#[cfg(feature = "renderer_gl")]
impl Bind<GlesTexture> for AutoRenderer {
    fn bind<'a>(&mut self, target: &'a mut GlesTexture) -> Result<Self::Framebuffer<'a>, Self::Error> {
        let AutoRenderer::Gles(renderer) = self else {
            return Err(AutoRendererError::Unsupported);
        };
        Bind::<GlesTexture>::bind(renderer, target)
            .map(AutoRendererTarget::from)
            .map_err(AutoRendererError::from)
    }

    fn supported_formats(&self) -> Option<FormatSet> {
        let AutoRenderer::Gles(renderer) = self else {
            return None;
        };
        Bind::<GlesTexture>::supported_formats(renderer)
    }
}

#[cfg(feature = "renderer_gl")]
impl Bind<GlesRenderbuffer> for AutoRenderer {
    fn bind<'a>(&mut self, target: &'a mut GlesRenderbuffer) -> Result<Self::Framebuffer<'a>, Self::Error> {
        let AutoRenderer::Gles(renderer) = self else {
            return Err(AutoRendererError::Unsupported);
        };
        Bind::<GlesRenderbuffer>::bind(renderer, target)
            .map(AutoRendererTarget::from)
            .map_err(AutoRendererError::from)
    }

    fn supported_formats(&self) -> Option<FormatSet> {
        let AutoRenderer::Gles(renderer) = self else {
            return None;
        };
        Bind::<GlesRenderbuffer>::supported_formats(renderer)
    }
}

#[cfg(feature = "renderer_gl")]
impl Bind<Image<'static, 'static>> for AutoRenderer {
    fn bind<'a>(
        &mut self,
        target: &'a mut Image<'static, 'static>,
    ) -> Result<Self::Framebuffer<'a>, Self::Error> {
        let AutoRenderer::Pixman(renderer) = self else {
            return Err(AutoRendererError::Unsupported);
        };
        Bind::<Image<'static, 'static>>::bind(renderer, target)
            .map(AutoRendererTarget::from)
            .map_err(AutoRendererError::from)
    }

    fn supported_formats(&self) -> Option<FormatSet> {
        let AutoRenderer::Pixman(renderer) = self else {
            return None;
        };
        Bind::<Image<'static, 'static>>::supported_formats(renderer)
    }
}
