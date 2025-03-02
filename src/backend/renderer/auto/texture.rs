use drm_fourcc::DrmFourcc;

use crate::{
    backend::renderer::Texture,
    utils::{Buffer, Size},
};

#[cfg(feature = "renderer_gl")]
use crate::backend::renderer::gles::GlesTexture;
#[cfg(feature = "renderer_pixman")]
use crate::backend::renderer::pixman::PixmanTexture;

use super::AutoRendererError;

/// Texture for the auto renderer
#[derive(Debug)]
pub enum AutoRendererTexture {
    /// Gles texture
    #[cfg(feature = "renderer_gl")]
    Gles(GlesTexture),
    /// Pixman texture
    #[cfg(feature = "renderer_pixman")]
    Pixman(PixmanTexture),
}

#[cfg(feature = "renderer_gl")]
impl From<GlesTexture> for AutoRendererTexture {
    fn from(value: GlesTexture) -> Self {
        Self::Gles(value)
    }
}

#[cfg(feature = "renderer_gl")]
impl<'a> TryInto<&'a GlesTexture> for &'a AutoRendererTexture {
    type Error = AutoRendererError;

    fn try_into(self) -> Result<&'a GlesTexture, Self::Error> {
        match self {
            AutoRendererTexture::Gles(texture) => Ok(texture),
            _ => Err(AutoRendererError::IncompatibleResource),
        }
    }
}

#[cfg(feature = "renderer_pixman")]
impl From<PixmanTexture> for AutoRendererTexture {
    fn from(value: PixmanTexture) -> Self {
        Self::Pixman(value)
    }
}

#[cfg(feature = "renderer_pixman")]
impl<'a> TryInto<&'a PixmanTexture> for &'a AutoRendererTexture {
    type Error = AutoRendererError;

    fn try_into(self) -> Result<&'a PixmanTexture, Self::Error> {
        match self {
            AutoRendererTexture::Pixman(texture) => Ok(texture),
            _ => Err(AutoRendererError::IncompatibleResource),
        }
    }
}

impl Texture for AutoRendererTexture {
    fn size(&self) -> Size<i32, Buffer> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererTexture::Gles(texture) => texture.size(),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererTexture::Pixman(texture) => texture.size(),
        }
    }

    fn width(&self) -> u32 {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererTexture::Gles(texture) => texture.width(),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererTexture::Pixman(texture) => texture.width(),
        }
    }

    fn height(&self) -> u32 {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererTexture::Gles(texture) => texture.height(),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererTexture::Pixman(texture) => texture.height(),
        }
    }

    fn format(&self) -> Option<DrmFourcc> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererTexture::Gles(texture) => texture.format(),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererTexture::Pixman(texture) => texture.format(),
        }
    }
}
