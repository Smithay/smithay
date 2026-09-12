//! Workspace

use std::sync::{Arc, Mutex};

use crate::utils::user_data::UserDataMap;

/// Handle to a Workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceHandle {
    inner: Arc<(Mutex<Workspace>, UserDataMap)>
}

/// Weak version of [`WorkspaceHandle`].
#[derive(Debug, Clone)]
pub struct WorkspaceWeakHandle {
    inner: std::sync::Weak<(Mutex<Workspace>, UserDataMap)>
}

/// Workspace internal data.
#[derive(Debug)]
pub(crate) struct Workspace {}

impl WorkspaceHandle {
    /// Creates a new [`WorkspaceWeakHandle`] pointing to the same Workspace.
    pub fn downgrade(&self) -> WorkspaceWeakHandle {
        WorkspaceWeakHandle {
            inner: Arc::downgrade(&self.inner)
        }
    }
}

impl WorkspaceWeakHandle {
    /// Attempts to upgrade the `WorkspaceWeakHandle` to a [`WorkspaceHandle`]
    pub fn upgrade(&self) -> Option<WorkspaceHandle> {
        self.inner.upgrade().map(|inner| WorkspaceHandle { inner })
    }
}
