#[cfg(feature = "renderer_gl")]
use crate::backend::renderer::gles::GlesError;
#[cfg(feature = "renderer_pixman")]
use crate::backend::renderer::pixman::PixmanError;

/// Error for the auto renderer
#[derive(Debug, thiserror::Error)]
pub enum AutoRendererError {
    /// Gles error
    #[cfg(feature = "renderer_gl")]
    #[error(transparent)]
    Gles(GlesError),
    /// Pixman error
    #[cfg(feature = "renderer_pixman")]
    #[error(transparent)]
    Pixman(PixmanError),
    /// Incompatible resource
    #[error("An incompatible resource has been passed")]
    IncompatibleResource,
    /// Unsupported
    #[error("The operation is not supported on this particular renderer")]
    Unsupported,
}

#[cfg(feature = "renderer_gl")]
impl From<GlesError> for AutoRendererError {
    fn from(value: GlesError) -> Self {
        Self::Gles(value)
    }
}

#[cfg(feature = "renderer_pixman")]
impl From<PixmanError> for AutoRendererError {
    fn from(value: PixmanError) -> Self {
        Self::Pixman(value)
    }
}
