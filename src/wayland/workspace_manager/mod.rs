//! Workspace protocol
//!
//! The purpose of this protocol is to enable the creation of taskbars and docks by providing them
//! with a list of workspaces and their properties, and allowing them to activate and deactivate
//! workspaces.

use std::sync::{Arc, Mutex, MutexGuard, atomic::Ordering};

use portable_atomic::AtomicBool;
use wayland_protocols::ext::workspace::v1::server::{
    ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
    ext_workspace_handle_v1::ExtWorkspaceHandleV1,
    ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
};
use wayland_server::{
    Client, Dispatch, DisplayHandle, GlobalDispatch, Resource, Weak,
    backend::{ClientId, GlobalId, ObjectId},
};

use crate::{
    utils::user_data::UserDataMap,
    wayland::{Dispatch2, GlobalDispatch2},
};

pub mod group;
use group::{PendingGroupRequest, WorkspaceGroup, WorkspaceGroupData};

pub mod workspace;
use workspace::{PendingWorkspaceRequest, Workspace, WorkspaceData};

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
    pub(crate) groups: Vec<WorkspaceGroup>,

    pub(crate) instances: Vec<ExtWorkspaceManagerV1>,
    pub(crate) done_needed: Arc<AtomicBool>,
}

/// Data associated with manager handle.
#[derive(Debug, Default)]
pub struct WorkspaceManagerObjectData {
    pub(crate) inner: Mutex<PrivateManagerData>,
}

#[derive(Debug, Default)]
pub(crate) struct PrivateManagerData {
    pub(crate) pending_requests: Vec<PendingRequest>,
    public_data: WorkspaceManagerData,
}

/// The state container associated with a [`ExtWorkspaceManagerV1`].
///
/// This container provides 2 main storages:
/// - the `data_map` storage has typemap semantics and allows you to associate and arbitrary data.
/// - the `requests` storage contains the current requests associated with the wayland object and is
///   filled on [WorkspaceHandler::commit], and flushed afterward.
#[derive(Debug, Default)]
pub struct WorkspaceManagerData {
    /// The typemap storage of this Manager.
    pub data_map: UserDataMap,
    /// Requests committed by the client.
    pub requests: Vec<Request>,
}

/// Requests from a given client.
#[derive(Debug)]
pub enum Request {
    /// The client requested that the compositor create a workspace associated with this group.
    CreateWorkspace {
        /// Target group for the workspace.
        group: ExtWorkspaceGroupHandleV1,
        /// Workspace name.
        workspace: String,
    },
    /// The client requested a workspace to be activated.
    Activate(ExtWorkspaceHandleV1),
    /// The client requested a workspace to be deactivated.
    Deactivate(ExtWorkspaceHandleV1),
    /// the client requested a workspace to be assigned to a workspace group.
    Assign {
        /// Workspace to assign
        workspace: ExtWorkspaceHandleV1,
        /// Group to assign the workspace to.
        group: ExtWorkspaceGroupHandleV1,
    },
    /// The client requested the workspace to be removed.
    Remove(ExtWorkspaceHandleV1),
}

/// Data associated with a [ExtWorkspaceManagerV1] global.
#[allow(missing_debug_implementations)]
pub struct WorkspaceManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

#[derive(Debug)]
pub(crate) enum PendingRequest {
    Group(Weak<ExtWorkspaceGroupHandleV1>, PendingGroupRequest),
    Workspace(Weak<ExtWorkspaceHandleV1>, PendingWorkspaceRequest),
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
            display: display.clone(),

            workspaces: Default::default(),
            groups: Default::default(),

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
    pub fn done(&mut self) {
        let need_done = self.done_needed.swap(false, Ordering::Acquire);

        if need_done {
            for instance in &self.instances {
                instance.done();
            }
        }
    }

    fn remove_instance(&mut self, manager: &ExtWorkspaceManagerV1, with_events: bool) {
        if let Some(pos) = self.instances.iter().position(|i| i == manager) {
            let manager = self.instances.remove(pos);
            WorkspaceManagerObjectData::cleanup(&manager);

            // We remove groups first as they will emit the workspace_leave event.
            for group in &self.groups {
                group.remove_instance(&manager, with_events);
            }

            for workspace in &self.workspaces {
                workspace.remove_instance(&manager, with_events);
            }

            if with_events {
                manager.finished();
            }
        }
    }
}

impl<D> GlobalDispatch2<ExtWorkspaceManagerV1, D> for WorkspaceManagerGlobalData
where
    D: Dispatch<ExtWorkspaceManagerV1, WorkspaceManagerObjectData>,
    D: Dispatch<ExtWorkspaceHandleV1, WorkspaceData>,
    D: Dispatch<ExtWorkspaceGroupHandleV1, WorkspaceGroupData>,
    D: WorkspaceHandler,
{
    fn bind(
        &self,
        state: &mut D,
        handle: &DisplayHandle,
        client: &wayland_server::Client,
        resource: wayland_server::New<ExtWorkspaceManagerV1>,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let instance = data_init.init(resource, Default::default());
        let state = state.workspace_manager_state();

        for workspace in &state.workspaces {
            workspace.new_instance::<D>(client, handle, &instance);
        }

        for group in &state.groups {
            group.new_instance::<D>(client, handle, &instance);
        }

        instance.done();
        state.instances.push(instance);
    }

    fn can_view(&self, client: &wayland_server::Client) -> bool {
        (self.filter)(client)
    }
}

impl<D> Dispatch2<ExtWorkspaceManagerV1, D> for WorkspaceManagerObjectData
where
    D: WorkspaceHandler,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        manager: &ExtWorkspaceManagerV1,
        request: <ExtWorkspaceManagerV1 as wayland_server::Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            ext_workspace_manager_v1::Request::Stop => {
                state.workspace_manager_state().remove_instance(manager, true);
            }
            ext_workspace_manager_v1::Request::Commit => {
                Self::commit(state, manager);
            }
            _ => {}
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, manager: &ExtWorkspaceManagerV1) {
        state.workspace_manager_state().remove_instance(manager, false);
    }
}

impl PendingRequest {
    fn commit(self) -> Option<Request> {
        match self {
            Self::Group(weak_group, PendingGroupRequest::CreateWorkspace(workspace)) => weak_group
                .upgrade()
                .ok()
                .map(|group| Request::CreateWorkspace { group, workspace }),
            Self::Workspace(weak_workspace, workspace_request) => {
                let workspace = weak_workspace.upgrade().ok()?;

                match workspace_request {
                    PendingWorkspaceRequest::Activate => Some(Request::Activate(workspace)),
                    PendingWorkspaceRequest::Deactivate => Some(Request::Deactivate(workspace)),
                    PendingWorkspaceRequest::Assign(weak_group) => weak_group
                        .upgrade()
                        .ok()
                        .map(|group| Request::Assign { workspace, group }),
                    PendingWorkspaceRequest::Remove => Some(Request::Remove(workspace)),
                }
            }
        }
    }
}

impl WorkspaceManagerObjectData {
    fn lock_user_data(manager: &ExtWorkspaceManagerV1) -> MutexGuard<'_, PrivateManagerData> {
        manager.data::<Self>().unwrap().inner.lock().unwrap()
    }

    fn with_state<F, T>(manager: &ExtWorkspaceManagerV1, f: F) -> T
    where
        F: FnOnce(&WorkspaceManagerData) -> T,
    {
        let guard = Self::lock_user_data(manager);
        f(&guard.public_data)
    }

    pub(crate) fn add_pending(manager: &ExtWorkspaceManagerV1, request: PendingRequest) {
        let mut guard = Self::lock_user_data(manager);
        guard.pending_requests.push(request);
    }

    pub(crate) fn remove_pending(manager: &ExtWorkspaceManagerV1, source_id: ObjectId) {
        let mut guard = Self::lock_user_data(manager);

        guard.pending_requests.retain(|rq| match rq {
            PendingRequest::Workspace(weak, _) => weak.id() != source_id,
            PendingRequest::Group(weak, _) => weak.id() != source_id,
        });
    }

    pub(crate) fn cleanup(manager: &ExtWorkspaceManagerV1) {
        let mut guard = Self::lock_user_data(manager);

        std::mem::take(&mut guard.pending_requests);
        std::mem::take(&mut guard.public_data.requests);
    }

    fn commit<D>(state: &mut D, manager: &ExtWorkspaceManagerV1)
    where
        D: WorkspaceHandler,
    {
        {
            let mut guard = Self::lock_user_data(manager);

            let requests = guard
                .pending_requests
                .drain(..)
                .filter_map(PendingRequest::commit);
            guard.public_data.requests = requests.collect();
        }

        state.commit(manager);

        let mut guard = Self::lock_user_data(manager);
        std::mem::take(&mut guard.public_data.requests);
    }
}

/// Access the state associated to this manager instance.
pub fn with_state<F, T>(manager: &ExtWorkspaceManagerV1, f: F) -> T
where
    F: FnOnce(&WorkspaceManagerData) -> T,
{
    WorkspaceManagerObjectData::with_state(manager, f)
}
