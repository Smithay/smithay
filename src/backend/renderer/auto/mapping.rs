use drm_fourcc::DrmFourcc;

use crate::{
    backend::renderer::{Texture, TextureMapping},
    utils::{Buffer, Size},
};

#[cfg(feature = "renderer_gl")]
use crate::backend::renderer::gles::GlesMapping;
#[cfg(feature = "renderer_pixman")]
use crate::backend::renderer::pixman::PixmanMapping;

use super::AutoRendererError;

/// Mapping for the auto renderer
#[derive(Debug)]
pub enum AutoRendererMapping {
    /// Gles mapping
    #[cfg(feature = "renderer_gl")]
    Gles(GlesMapping),
    /// Pixman mapping
    #[cfg(feature = "renderer_pixman")]
    Pixman(PixmanMapping),
}

#[cfg(feature = "renderer_gl")]
impl From<GlesMapping> for AutoRendererMapping {
    fn from(value: GlesMapping) -> Self {
        Self::Gles(value)
    }
}

#[cfg(feature = "renderer_gl")]
impl TryFrom<AutoRendererMapping> for GlesMapping {
    type Error = AutoRendererError;

    fn try_from(value: AutoRendererMapping) -> Result<Self, Self::Error> {
        match value {
            AutoRendererMapping::Gles(mapping) => Ok(mapping),
            _ => Err(AutoRendererError::IncompatibleResource),
        }
    }
}

#[cfg(feature = "renderer_gl")]
impl<'a> TryFrom<&'a AutoRendererMapping> for &'a GlesMapping {
    type Error = AutoRendererError;

    fn try_from(value: &'a AutoRendererMapping) -> Result<Self, Self::Error> {
        match value {
            AutoRendererMapping::Gles(mapping) => Ok(mapping),
            _ => Err(AutoRendererError::IncompatibleResource),
        }
    }
}

#[cfg(feature = "renderer_pixman")]
impl From<PixmanMapping> for AutoRendererMapping {
    fn from(value: PixmanMapping) -> Self {
        Self::Pixman(value)
    }
}

#[cfg(feature = "renderer_pixman")]
impl TryFrom<AutoRendererMapping> for PixmanMapping {
    type Error = AutoRendererError;

    fn try_from(value: AutoRendererMapping) -> Result<Self, Self::Error> {
        match value {
            AutoRendererMapping::Pixman(mapping) => Ok(mapping),
            _ => Err(AutoRendererError::IncompatibleResource),
        }
    }
}

#[cfg(feature = "renderer_pixman")]
impl<'a> TryFrom<&'a AutoRendererMapping> for &'a PixmanMapping {
    type Error = AutoRendererError;

    fn try_from(value: &'a AutoRendererMapping) -> Result<Self, Self::Error> {
        match value {
            AutoRendererMapping::Pixman(mapping) => Ok(mapping),
            _ => Err(AutoRendererError::IncompatibleResource),
        }
    }
}

impl Texture for AutoRendererMapping {
    fn size(&self) -> Size<i32, Buffer> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererMapping::Gles(texture) => Texture::size(texture),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererMapping::Pixman(texture) => Texture::size(texture),
        }
    }

    fn width(&self) -> u32 {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererMapping::Gles(texture) => Texture::width(texture),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererMapping::Pixman(texture) => Texture::width(texture),
        }
    }

    fn height(&self) -> u32 {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererMapping::Gles(texture) => Texture::height(texture),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererMapping::Pixman(texture) => Texture::height(texture),
        }
    }

    fn format(&self) -> Option<DrmFourcc> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererMapping::Gles(texture) => Texture::format(texture),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererMapping::Pixman(texture) => Texture::format(texture),
        }
    }
}

impl TextureMapping for AutoRendererMapping {
    fn flipped(&self) -> bool {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererMapping::Gles(texture) => TextureMapping::flipped(texture),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererMapping::Pixman(texture) => TextureMapping::flipped(texture),
        }
    }

    fn format(&self) -> gbm::Format {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererMapping::Gles(texture) => TextureMapping::format(texture),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererMapping::Pixman(texture) => TextureMapping::format(texture),
        }
    }
}
