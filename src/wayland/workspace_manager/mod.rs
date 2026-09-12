//! Workspace protocol
//!
//! The purpose of this protocol is to enable the creation of taskbars and docks by providing them
//! with a list of workspaces and their properties, and allowing them to activate and deactivate
//! workspaces.

use std::sync::{Arc, Mutex, atomic::Ordering};

use portable_atomic::AtomicBool;
use wayland_protocols::ext::workspace::v1::server::{
    ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
    ext_workspace_handle_v1::ExtWorkspaceHandleV1,
    ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
};
use wayland_server::{
    Client, Dispatch, DisplayHandle, GlobalDispatch,
    backend::{ClientId, GlobalId},
};

use crate::wayland::{Dispatch2, GlobalDispatch2};

const MANAGER_VERSION: u32 = 1;

/// Handler for workspace protocol
pub trait WorkspaceHandler: 'static {
    /// [`WorkspaceManagerState`] accessor
    fn workspace_manager_state(&mut self) -> &mut WorkspaceManagerState;

    /// A client committed some requests.
    fn commit(&mut self, manager: ExtWorkspaceManagerV1);

    /// A client requested to create a new workspace.
    ///
    /// This function is meant as a hook for compositor that want to extends the protocol.
    fn create_workspace(&mut self, _group: ExtWorkspaceGroupHandleV1, _workspace: String) {}

    /// A client requested a workspace to be activated.
    fn activate(&mut self, _workspace: ExtWorkspaceHandleV1) {}

    /// A client requested a workspace to be deactivated.
    fn deactivate(&mut self, _workspace: ExtWorkspaceHandleV1) {}

    /// A client requested a workspace to be assigned to a group.
    fn assign(&mut self, _workspace: ExtWorkspaceHandleV1, _group: ExtWorkspaceGroupHandleV1) {}

    /// A client requested a workspace to be removed.
    fn remove(&mut self, _workspace: ExtWorkspaceHandleV1) {}
}

pub(crate) type WorkspaceManagerData = Mutex<WorkspaceManagerDataInner>;
#[derive(Debug, Default)]
pub(crate) struct WorkspaceManagerDataInner {
    //TODO: We need a way to store request such that if the client destroyed the original object,
    //we're not sending anything to it. One way to do so would be to discard the request altogether.
}

/// Data associated with a [ExtWorkspaceManagerV1] global.
#[allow(missing_debug_implementations)]
pub struct WorkspaceManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

/// Workspace Manager State
#[derive(Debug)]
pub struct WorkspaceManagerState {
    global: GlobalId,
    pub(crate) _display: DisplayHandle,

    pub(crate) instances: Vec<ExtWorkspaceManagerV1>,
    pub(crate) done_needed: Arc<AtomicBool>,
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
                filter: Box::new(filter),
            },
        );

        Self {
            global,
            _display: display.clone(),
            instances: Default::default(),
            done_needed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Gets [ExtWorkspaceManagerV1] global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }

    /// Terminates a changes transaction.
    ///
    /// This function should be called after all changes in all workspaces and workspace group have
    /// been sent.
    ///
    /// This allows changes to one or more group or workspace properties to be seen as atomic.
    pub fn done(&self) {
        let need_done = self.done_needed.swap(false, Ordering::Acquire);

        if need_done {
            for instance in &self.instances {
                instance.done();
            }
        }
    }
}

impl<D> GlobalDispatch2<ExtWorkspaceManagerV1, D> for WorkspaceManagerGlobalData
where
    D: Dispatch<ExtWorkspaceManagerV1, WorkspaceManagerData>,
    D: WorkspaceHandler,
{
    fn bind(
        &self,
        state: &mut D,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: wayland_server::New<ExtWorkspaceManagerV1>,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let instance = data_init.init(resource, Default::default());
        let state = state.workspace_manager_state();

        // We send the workspace first, as that simplifies group handling.
        //TODO: Send workspaces

        //TODO: Send groups
        //TODO: Send group outputs
        //TODO: Send group workspaces

        state.instances.push(instance);
    }

    fn can_view(&self, client: &wayland_server::Client) -> bool {
        (self.filter)(client)
    }
}

impl<D> Dispatch2<ExtWorkspaceManagerV1, D> for WorkspaceManagerData
where
    D: WorkspaceHandler,
{
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        manager: &ExtWorkspaceManagerV1,
        request: <ExtWorkspaceManagerV1 as wayland_server::Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            ext_workspace_manager_v1::Request::Stop => {
                self.destroyed(state, client.id(), manager);
                manager.finished();
            }
            ext_workspace_manager_v1::Request::Commit => {
                // TODO: Call commit, then flush pending requests, if any.
            }
            _ => {}
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &ExtWorkspaceManagerV1) {
        state
            .workspace_manager_state()
            .instances
            .retain(|i| i != resource);
    }
}
