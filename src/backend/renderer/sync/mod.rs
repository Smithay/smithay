//! Helper for synchronizing rendering operations
use std::{error::Error, fmt, os::unix::io::OwnedFd, sync::Arc};

use downcast_rs::{impl_downcast, Downcast};

#[cfg(feature = "backend_egl")]
mod egl;

/// Waiting for the fence was interrupted for an unknown reason.
///
/// This does not mean that the fence is signalled or not, neither that
/// any timeout was reached. Waiting should be attempted again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Interrupted;

impl fmt::Display for Interrupted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Wait for Fence was interrupted")
    }
}
impl Error for Interrupted {}

/// A fence that will be signaled in finite time
pub trait Fence: std::fmt::Debug + Send + Sync + Downcast {
    /// Queries the state of the fence
    fn is_signaled(&self) -> bool;

    /// Blocks the current thread until the fence is signaled
    fn wait(&self) -> Result<(), Interrupted>;

    /// Returns whether this fence can be exported
    /// as a native fence fd
    fn is_exportable(&self) -> bool;

    /// Export this fence as a native fence fd
    fn export(&self) -> Option<OwnedFd>;
}
impl_downcast!(Fence);

/// A sync point the will be signaled in finite time
#[derive(Debug, Clone)]
#[must_use = "this `SyncPoint` may contain a fence that should be awaited, failing to do so may result in unexpected rendering artifacts"]
pub struct SyncPoint {
    fence: Option<Arc<dyn Fence>>,
}

impl Default for SyncPoint {
    fn default() -> Self {
        Self::signaled()
    }
}

impl SyncPoint {
    /// Create an already signaled sync point
    pub fn signaled() -> Self {
        Self {
            fence: Default::default(),
        }
    }

    /// Get a reference to the underlying [`Fence`] if any
    ///
    /// Returns `None` if the sync point does not contain a fence
    /// or contains a different type of fence
    pub fn get<F: Fence + 'static>(&self) -> Option<&F> {
        self.fence.as_ref().and_then(|f| f.downcast_ref())
    }

    /// Queries the state of the sync point
    ///
    /// Will always return `true` in case the sync point does not contain a fence
    pub fn is_reached(&self) -> bool {
        self.fence.as_ref().map(|f| f.is_signaled()).unwrap_or(true)
    }

    /// Blocks the current thread until the sync point is signaled
    ///
    /// If the sync point does not contain a fence this will never block.
    #[profiling::function]
    pub fn wait(&self) -> Result<(), Interrupted> {
        if let Some(fence) = self.fence.as_ref() {
            fence.wait()
        } else {
            Ok(())
        }
    }

    /// Returns whether this sync point can be exported as a native fence fd
    ///
    /// Will always return `false` in case the sync point does not contain a fence
    pub fn is_exportable(&self) -> bool {
        self.fence.as_ref().map(|f| f.is_exportable()).unwrap_or(false)
    }

    /// Export this [`SyncPoint`] as a native fence fd
    ///
    /// Will always return `None` in case the sync point does not contain a fence
    #[profiling::function]
    pub fn export(&self) -> Option<OwnedFd> {
        self.fence.as_ref().and_then(|f| f.export())
    }
}

impl<T: Fence + 'static> From<T> for SyncPoint {
    fn from(value: T) -> Self {
        SyncPoint {
            fence: Some(Arc::new(value)),
        }
    }
}
