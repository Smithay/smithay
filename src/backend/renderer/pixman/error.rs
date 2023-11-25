use drm_fourcc::{DrmFourcc, DrmModifier};
use thiserror::Error;

use crate::backend::{
    allocator::dmabuf::{DmabufMappingFailed, DmabufSyncFailed},
    SwapBuffersError,
};

#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::wl_shm;

/// Error returned during rendering using pixman
#[derive(Debug, Error)]
pub enum PixmanError {
    /// The given buffer has an unsupported number of planes
    #[error("Unsupported number of planes")]
    UnsupportedNumberOfPlanes,
    /// The given buffer has an unsupported pixel format
    #[error("Unsupported pixel format: {0:?}")]
    UnsupportedPixelFormat(DrmFourcc),
    /// The given buffer has an unsupported modifier
    #[error("Unsupported modifier: {0:?}")]
    UnsupportedModifier(DrmModifier),
    /// The given wl buffer has an unsupported pixel format
    #[error("Unsupported wl_shm format: {0:?}")]
    #[cfg(feature = "wayland_frontend")]
    UnsupportedWlPixelFormat(wl_shm::Format),
    /// The given buffer is incomplete
    #[error("Incomplete buffer {expected} < {actual}")]
    IncompleteBuffer {
        /// Expected len of the buffer
        expected: usize,
        /// Actual len of the buffer
        actual: usize,
    },
    /// The given buffer was not accessible
    #[error("Error accessing the buffer ({0:?})")]
    #[cfg(feature = "wayland_frontend")]
    BufferAccessError(#[from] crate::wayland::shm::BufferAccessError),
    /// Failed to import the given buffer
    #[error("Import failed")]
    ImportFailed,
    /// The underlying buffer has been destroyed
    #[error("The underlying buffer has been destroyed")]
    BufferDestroyed,
    /// Mapping the buffer failed
    #[error("Mapping the buffer failed: {0}")]
    Map(#[from] DmabufMappingFailed),
    /// Synchronizing access to the buffer failed
    #[error("Synchronizing buffer failed: {0}")]
    Sync(#[from] DmabufSyncFailed),
    /// The requested operation failed
    #[error("The requested operation failed")]
    Failed(#[from] pixman::OperationFailed),
    /// No target is currently bound
    #[error("No target is currently bound")]
    NoTargetBound,
    /// The requested operation is not supported
    #[error("The requested operation is not supported")]
    Unsupported,
}

impl From<PixmanError> for SwapBuffersError {
    fn from(value: PixmanError) -> Self {
        SwapBuffersError::ContextLost(Box::new(value))
    }
}
