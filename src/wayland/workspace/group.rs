//! Workspace Group

use std::sync::{Arc, Mutex};

use crate::utils::user_data::UserDataMap;

/// A handle to a workspace group.
#[derive(Debug, Clone)]
pub struct WorkspaceGroupHandle {
    inner: Arc<(Mutex<WorkspaceGroup>, UserDataMap)>
}

/// Weak version of [`WorkspaceGroupHandle`].
#[derive(Debug, Clone)]
pub struct WorkspaceGroupWeakHandle {
    inner: std::sync::Weak<(Mutex<WorkspaceGroup>, UserDataMap)>
}

/// Workspace internal data.
#[derive(Debug)]
pub(crate) struct WorkspaceGroup {}

impl WorkspaceGroupHandle {
    /// Creates a new [`WorkspaceGroupWeakHandle`] pointing to the same workspace group.
    pub fn downgrade(&self) -> WorkspaceGroupWeakHandle {
        WorkspaceGroupWeakHandle {
            inner: Arc::downgrade(&self.inner)
        }
    }
}

impl WorkspaceGroupWeakHandle {
    /// Attempts to upgrade the `WorkspaceGroupWeakHandle` to a [`WorkspaceGroupHandle`]
    pub fn upgrade(&self) -> Option<WorkspaceGroupHandle> {
        self.inner.upgrade().map(|inner| WorkspaceGroupHandle { inner })
    }
}
