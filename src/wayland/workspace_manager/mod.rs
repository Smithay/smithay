//! Workspace protocol
//!
//! The purpose of this protocol is to enable the creation of taskbars and docks by providing them
//! with a list of workspaces and their properties, and allowing them to activate and deactivate
//! workspaces.

use wayland_protocols::ext::workspace::v1::server::{
    ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1, ext_workspace_handle_v1::ExtWorkspaceHandleV1,
    ext_workspace_manager_v1::ExtWorkspaceManagerV1,
};
use wayland_server::{Client, DisplayHandle, GlobalDispatch, backend::GlobalId};

pub mod workspace;
use workspace::Workspace;

const MANAGER_VERSION: u32 = 1;

/// Handler for workspace protocol
#[allow(unused_variables)]
pub trait WorkspaceHandler: 'static {
    /// [`WorkspaceManagerState`] accessor
    fn workspace_manager_state(&mut self) -> &mut WorkspaceManagerState;

    /// A client committed some requests.
    fn commit(&mut self, manager: &ExtWorkspaceManagerV1);

    /// A client requested to create a new workspace.
    fn create_workspace(&mut self, group: &ExtWorkspaceGroupHandleV1, workspace: String) {}

    /// A client requested a workspace to be activated.
    fn activate(&mut self, workspace: &ExtWorkspaceHandleV1) {}

    /// A client requested a workspace to be deactivated.
    fn deactivate(&mut self, workspace: &ExtWorkspaceHandleV1) {}

    /// A client requested a workspace to be assigned to a group.
    fn assign(&mut self, workspace: &ExtWorkspaceHandleV1, group: &ExtWorkspaceGroupHandleV1) {}

    /// A client requested a workspace to be removed.
    fn remove(&mut self, workspace: &ExtWorkspaceHandleV1) {}

    /// A workspace group was destroyed.
    fn group_destroyed(&mut self, group: &ExtWorkspaceGroupHandleV1) {}

    /// A workspace was destroyed.
    fn workspace_destroyed(&mut self, workspace: &ExtWorkspaceHandleV1) {}
}

/// Workspace Manager State
#[derive(Debug)]
pub struct WorkspaceManagerState {
    global: GlobalId,
    pub(crate) display: DisplayHandle,

    pub(crate) workspaces: Vec<Workspace>,

    pub(crate) instances: Vec<ExtWorkspaceManagerV1>,
}

/// Data associated with a [ExtWorkspaceManagerV1] global.
#[allow(missing_debug_implementations)]
pub struct WorkspaceManagerGlobalData {
    _filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl WorkspaceManagerState {
    /// Registers a new [ExtWorkspaceManagerV1] global
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ExtWorkspaceManagerV1, WorkspaceManagerGlobalData>,
        D: WorkspaceHandler,
    {
        Self::new_with_filter::<D>(display, |_| true)
    }

    /// Registers new [ExtWorkspaceManagerV1] global with a filter.
    ///
    /// The `filter` parameter determines which clients will see the global.
    pub fn new_with_filter<D>(
        display: &DisplayHandle,
        filter: impl Fn(&Client) -> bool + Send + Sync + 'static,
    ) -> Self
    where
        D: GlobalDispatch<ExtWorkspaceManagerV1, WorkspaceManagerGlobalData>,
        D: WorkspaceHandler,
    {
        let global = display.create_global::<D, ExtWorkspaceManagerV1, _>(
            MANAGER_VERSION,
            WorkspaceManagerGlobalData {
                _filter: Box::new(filter),
            },
        );

        Self {
            global,
            display: display.clone(),

            workspaces: Default::default(),

            instances: Default::default(),
        }
    }

    /// Gets [ExtWorkspaceManagerV1] global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }

    /// Notify clients that all changes were made.
    ///
    /// This function should be called after all changes in all workspaces and workspace group have
    /// been sent.
    ///
    /// When calling [`done`] directly, compositor should first call [`send_pending_changes`] to
    /// ensure the state was properly applied. Compositor that do not extend the protocol should
    /// call [`refresh`] instead, as it will ensure a proper ordering of operation.
    ///
    /// [`done`]: WorkspaceManagerState::done
    /// [`send_pending_changes`]: WorkspaceManagerState::send_pending_changes
    /// [`refresh`]: WorkspaceManagerState::refresh
    pub fn done(&mut self) {
        for instance in &self.instances {
            instance.done();
        }
    }

    /// Send all pending changes to clients, if any.
    ///
    /// This function sends all pending changes to client, and returns whether they were changes to
    /// send. After calling this function, [`done`] should be called to notify clients that all
    /// changes were made.
    ///
    /// [`done`]: WorkspaceManagerState::done
    pub fn send_pending_changes(&mut self) -> bool {
        let mut changed = false;

        changed |= self.send_workspace_changes();

        changed
    }

    /// Send all pending changes and notify clients that all changes have been sent.
    ///
    /// This function is equivalent to calling [`send_pending_changes`] followed by [`done`] if
    /// there were any. Compositor that don't extend the workspace protocol can simply call this
    /// regularly. The returns value indicate whether there were pending changes.
    ///
    /// [`send_pending_changes`]: WorkspaceManagerState::send_pending_changes
    /// [`done`]: WorkspaceManagerState::done
    pub fn refresh(&mut self) -> bool {
        let changed = self.send_pending_changes();

        if changed {
            self.done();
        }

        changed
    }
}
