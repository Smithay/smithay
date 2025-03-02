#[cfg(feature = "renderer_gl")]
use crate::backend::renderer::gles::GlesTarget;
#[cfg(feature = "renderer_pixman")]
use crate::backend::renderer::pixman::PixmanTarget;

use super::AutoRendererError;

/// Target for the auto-renderer
#[derive(Debug)]
pub enum AutoRendererTarget<'buffer> {
    /// Gles target
    #[cfg(feature = "renderer_gl")]
    Gles(GlesTarget<'buffer>),
    /// Pixman target
    #[cfg(feature = "renderer_pixman")]
    Pixman(PixmanTarget<'buffer>),
}

#[cfg(feature = "renderer_gl")]
impl<'buffer> From<GlesTarget<'buffer>> for AutoRendererTarget<'buffer> {
    fn from(value: GlesTarget<'buffer>) -> Self {
        Self::Gles(value)
    }
}

#[cfg(feature = "renderer_gl")]
impl<'buffer> TryFrom<AutoRendererTarget<'buffer>> for GlesTarget<'buffer> {
    type Error = AutoRendererError;

    fn try_from(value: AutoRendererTarget<'buffer>) -> Result<Self, Self::Error> {
        match value {
            AutoRendererTarget::Gles(target) => Ok(target),
            _ => Err(AutoRendererError::IncompatibleResource),
        }
    }
}

#[cfg(feature = "renderer_gl")]
impl<'a, 'buffer> TryFrom<&'a AutoRendererTarget<'buffer>> for &'a GlesTarget<'buffer> {
    type Error = AutoRendererError;

    fn try_from(value: &'a AutoRendererTarget<'buffer>) -> Result<Self, Self::Error> {
        match value {
            AutoRendererTarget::Gles(target) => Ok(target),
            _ => Err(AutoRendererError::IncompatibleResource),
        }
    }
}

#[cfg(feature = "renderer_gl")]
impl<'a, 'buffer> TryFrom<&'a mut AutoRendererTarget<'buffer>> for &'a mut GlesTarget<'buffer> {
    type Error = AutoRendererError;

    fn try_from(value: &'a mut AutoRendererTarget<'buffer>) -> Result<Self, Self::Error> {
        match value {
            AutoRendererTarget::Gles(target) => Ok(target),
            _ => Err(AutoRendererError::IncompatibleResource),
        }
    }
}

#[cfg(feature = "renderer_pixman")]
impl<'buffer> From<PixmanTarget<'buffer>> for AutoRendererTarget<'buffer> {
    fn from(value: PixmanTarget<'buffer>) -> Self {
        Self::Pixman(value)
    }
}

#[cfg(feature = "renderer_pixman")]
impl<'buffer> TryFrom<AutoRendererTarget<'buffer>> for PixmanTarget<'buffer> {
    type Error = AutoRendererError;

    fn try_from(value: AutoRendererTarget<'buffer>) -> Result<Self, Self::Error> {
        match value {
            AutoRendererTarget::Pixman(target) => Ok(target),
            _ => Err(AutoRendererError::IncompatibleResource),
        }
    }
}

#[cfg(feature = "renderer_pixman")]
impl<'a, 'buffer> TryFrom<&'a AutoRendererTarget<'buffer>> for &'a PixmanTarget<'buffer> {
    type Error = AutoRendererError;

    fn try_from(value: &'a AutoRendererTarget<'buffer>) -> Result<Self, Self::Error> {
        match value {
            AutoRendererTarget::Pixman(target) => Ok(target),
            _ => Err(AutoRendererError::IncompatibleResource),
        }
    }
}

#[cfg(feature = "renderer_pixman")]
impl<'a, 'buffer> TryFrom<&'a mut AutoRendererTarget<'buffer>> for &'a mut PixmanTarget<'buffer> {
    type Error = AutoRendererError;

    fn try_from(value: &'a mut AutoRendererTarget<'buffer>) -> Result<Self, Self::Error> {
        match value {
            AutoRendererTarget::Pixman(target) => Ok(target),
            _ => Err(AutoRendererError::IncompatibleResource),
        }
    }
}
