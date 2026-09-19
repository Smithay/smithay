//! Workspace Group

use std::{
    hash::Hash,
    ops::Deref,
    sync::{Arc, Mutex, atomic::Ordering},
};

use portable_atomic::AtomicBool;
use wayland_protocols::ext::workspace::v1::server::{
    ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1, GroupCapabilities},
    ext_workspace_manager_v1::ExtWorkspaceManagerV1,
};
use wayland_server::{Client, Dispatch, DisplayHandle, Resource, Weak, backend::ClientId};

use crate::{
    output::{Output, WeakOutput},
    utils::user_data::UserDataMap,
    wayland::{
        Dispatch2,
        workspace_manager::{
            WorkspaceHandler, WorkspaceManagerState,
            workspace::{self, WeakWorkspace, Workspace},
        },
    },
};

const GROUP_VERSION: u32 = 1;

/// A handle to a workspace group.
#[derive(Debug, Clone)]
pub struct WorkspaceGroup {
    inner: Arc<(Mutex<Inner>, UserDataMap)>,
}

/// Weak version of [`WorkspaceGroup`].
#[derive(Debug, Clone)]
pub struct WeakWorkspaceGroup {
    inner: std::sync::Weak<(Mutex<Inner>, UserDataMap)>,
}

/// Workspace internal data.
#[derive(Debug)]
pub(crate) struct Inner {
    capabilities: GroupCapabilities,

    outputs: Vec<WeakOutput>,
    workspaces: Vec<WeakWorkspace>,

    instances: Vec<ExtWorkspaceGroupHandleV1>,
    done_needed: Option<Arc<AtomicBool>>,
}

/// Data associated with workspace group handles.
#[derive(Debug)]
pub struct WorkspaceGroupData {
    handle: WeakWorkspaceGroup,
    manager: Weak<ExtWorkspaceManagerV1>,
}

impl WorkspaceGroup {
    fn new(capabilities: GroupCapabilities, capacity: usize, done_needed: Arc<AtomicBool>) -> Self {
        let inner = Arc::new((
            Mutex::new(Inner {
                capabilities,
                outputs: Default::default(),
                workspaces: Default::default(),
                instances: Vec::with_capacity(capacity),
                done_needed: Some(done_needed),
            }),
            UserDataMap::default(),
        ));

        Self { inner }
    }

    /// Gets the `WorkspaceGroup` associated with an [`ExtWorkspaceGroupHandleV1`].
    pub fn from_resource(resource: &ExtWorkspaceGroupHandleV1) -> Option<Self> {
        resource
            .data::<WorkspaceGroupData>()
            .and_then(|d| d.handle.upgrade())
    }

    /// Creates a new [`WeakWorkspaceGroup`] pointing to the same workspace group.
    pub fn downgrade(&self) -> WeakWorkspaceGroup {
        WeakWorkspaceGroup {
            inner: Arc::downgrade(&self.inner),
        }
    }

    /// Access the [`UserDataMap`] associated with this [`WorkspaceGroup`].
    pub fn user_data(&self) -> &UserDataMap {
        &self.inner.1
    }

    /// Gets the workspace group's capabilities.
    pub fn capabilities(&self) -> GroupCapabilities {
        self.inner.0.lock().unwrap().capabilities
    }

    /// Sets the workspace group capabilities.
    pub fn set_capabilities(&self, capabilities: GroupCapabilities) {
        self.inner.0.lock().unwrap().set_capabilities(capabilities);
    }

    /// Gets the list of [`Output`] in this group.
    pub fn outputs(&self) -> Vec<Output> {
        let inner = self.inner.0.lock().unwrap();
        inner.outputs.iter().filter_map(|o| o.upgrade()).collect()
    }

    /// Check whether an [`Output`] is in this group.
    pub fn has_output(&self, output: &Output) -> bool {
        let inner = self.inner.0.lock().unwrap();

        inner.outputs.iter().find(|o| o == &output).is_some()
    }

    /// Adds an [`Output`] to this `WorkspaceGroup`.
    ///
    /// If the output is already in the workspace group, this function does nothing.
    /// Otherwise, the output is added and the `output_enter` signal is sent to all instances.
    pub fn output_enter(&self, output: &Output) {
        self.inner.0.lock().unwrap().add_output(self, output);
    }

    /// Removes an [`Output`] to this `WorkspaceGroup`.
    ///
    /// If the output isn't in the workspace group, this function does nothing.
    /// Otherwise, the output is removed and the `output_leave` signal is sent to all instances.
    pub fn output_leave(&self, output: &Output) {
        self.inner.0.lock().unwrap().remove_output(self, output);
    }

    /// Gets the workspaces currently part of this `WorkspaceGroup`.
    pub fn workspaces(&self) -> Vec<Workspace> {
        let inner = self.inner.0.lock().unwrap();
        inner.workspaces.iter().filter_map(|w| w.upgrade()).collect()
    }

    /// Adds a [`Workspace`] to this `WorkspaceGroup`.
    ///
    /// If the `Workspace` is already in the group, this function does nothing.
    /// Otherwise, if the `Workspace` is already in a group, the `workspace_leave` signal will be
    /// sent to all instances, followed by the `workspace_enter` signal.
    pub fn workspace_enter(&self, workspace: &Workspace) {
        self.inner.0.lock().unwrap().add_workspace(self, workspace);
    }

    /// Remove a [`Workspace`] from this `WorkspaceGroup`.
    pub fn workspace_leave(&self, workspace: &Workspace) {
        self.inner.0.lock().unwrap().remove_workspace(self, workspace);
    }

    pub(super) fn new_instance<D>(
        &self,
        client: &Client,
        display: &DisplayHandle,
        manager: &ExtWorkspaceManagerV1,
    ) where
        D: Dispatch<ExtWorkspaceGroupHandleV1, WorkspaceGroupData>,
        D: 'static,
    {
        let Ok(instance) = client.create_resource::<ExtWorkspaceGroupHandleV1, _, D>(
            display,
            GROUP_VERSION,
            WorkspaceGroupData {
                handle: self.downgrade(),
                manager: manager.downgrade(),
            },
        ) else {
            return;
        };

        let mut inner = self.inner.0.lock().unwrap();

        for workspace in &inner.workspaces {
            let Some(workspace) = workspace.upgrade() else {
                continue;
            };

            let w_inner = workspace.inner.0.lock().unwrap();
            let Some(w_instance) = w_inner.instance_with_manager(&manager.id()) else {
                continue;
            };

            instance.workspace_enter(w_instance);
        }

        for output in &inner.outputs {
            let Some(output) = output.upgrade() else {
                continue;
            };

            for o_instance in output.client_outputs(client) {
                instance.output_enter(&o_instance);
            }
        }

        inner.instances.push(instance);
    }
}

impl WeakWorkspaceGroup {
    /// Attempts to upgrade the `WeakWorkspaceGroup` to a [`WorkspaceGroup`]
    pub fn upgrade(&self) -> Option<WorkspaceGroup> {
        self.inner.upgrade().map(|inner| WorkspaceGroup { inner })
    }
}

impl Inner {
    fn add_output(&mut self, _handle: &WorkspaceGroup, output: &Output) {
        if self.outputs.iter().any(|o| o == output) {
            return;
        }

        let mut dirty = false;
        for instance in &self.instances {
            let Some(client) = instance.client() else {
                continue;
            };

            for output in output.client_outputs(&client) {
                instance.output_enter(&output);
                dirty = true;
            }
        }

        if dirty {
            self.need_done();
        }

        // TODO: add marker to output so next time a client bind it, and output_enter can be sent.

        self.outputs.push(output.downgrade());
    }

    fn remove_output(&mut self, _handle: &WorkspaceGroup, output: &Output) {
        let Some(pos) = self.outputs.iter().position(|o| o == output) else {
            return;
        };
        self.outputs.remove(pos);

        let mut dirty = false;
        for instance in &self.instances {
            let Some(client) = instance.client() else {
                continue;
            };

            for output in output.client_outputs(&client) {
                instance.output_leave(&output);
                dirty = true;
            }
        }

        if dirty {
            self.need_done();
        }

        // TODO: remove output <-> workspace group marker.

        self.outputs.retain(|o| o != output);
    }

    fn add_workspace(&mut self, handle: &WorkspaceGroup, workspace: &Workspace) {
        let mut w_inner = workspace.inner.0.lock().unwrap();

        if let Some(group) = w_inner.group.as_ref().and_then(WeakWorkspaceGroup::upgrade) {
            if &group != handle {
                group
                    .inner
                    .0
                    .lock()
                    .unwrap()
                    .remove_workspace_inner(w_inner.deref());
            } else {
                return;
            }
        };

        let mut dirty = false;
        for instance in &self.instances {
            let data = instance.data::<WorkspaceGroupData>().unwrap();

            let manager_id = data.manager.id();

            let Some(w_instance) = w_inner.instance_with_manager(&manager_id) else {
                continue;
            };

            instance.workspace_enter(w_instance);
            dirty = true;
        }

        if dirty {
            self.need_done();
        }

        w_inner.group = Some(handle.downgrade());
        self.workspaces.push(workspace.downgrade());
    }

    fn remove_workspace(&mut self, handle: &WorkspaceGroup, workspace: &Workspace) {
        let mut w_inner = workspace.inner.0.lock().unwrap();

        if w_inner.group.take_if(|g| g == handle).is_none() {
            return;
        }

        self.remove_workspace_inner(w_inner.deref());

        self.workspaces.retain(|w| w != workspace);
    }

    fn remove_workspace_inner(&mut self, w_inner: &workspace::Inner) {
        let mut dirty = false;

        for instance in &self.instances {
            let data = instance.data::<WorkspaceGroupData>().unwrap();

            let manager_id = data.manager.id();

            let Some(w_instance) = w_inner.instance_with_manager(&manager_id) else {
                continue;
            };

            instance.workspace_leave(w_instance);
            dirty = true;
        }

        if dirty {
            self.need_done();
        }
    }

    fn set_capabilities(&mut self, capabilities: GroupCapabilities) {
        if self.capabilities != capabilities {
            self.capabilities = capabilities;

            for instance in &self.instances {
                instance.capabilities(capabilities);
            }

            self.need_done();
        }
    }

    #[inline]
    fn need_done(&mut self) {
        if let Some(done_needed) = self.done_needed.as_mut() {
            done_needed.store(true, Ordering::Release);
        }
    }
}

impl WorkspaceManagerState {
    /// Creates a new [`WorkspaceGroup`] with defaults capabilities.
    pub fn new_group<D>(&mut self) -> WorkspaceGroup
    where
        D: Dispatch<ExtWorkspaceGroupHandleV1, WorkspaceGroupData>,
        D: 'static,
    {
        self.new_group_with_capabilities::<D>(GroupCapabilities::all())
    }

    /// Creates a new [`WorkspaceGroup`] with a given set of capabilities.
    pub fn new_group_with_capabilities<D>(&mut self, capabilities: GroupCapabilities) -> WorkspaceGroup
    where
        D: Dispatch<ExtWorkspaceGroupHandleV1, WorkspaceGroupData>,
        D: 'static,
    {
        let group = WorkspaceGroup::new(capabilities, self.instances.len(), self.done_needed.clone());

        for instance in &self.instances {
            let Ok(client) = self.display.get_client(instance.id()) else {
                continue;
            };

            group.new_instance::<D>(&client, &self.display, instance);
        }

        self.groups.push(group.clone());
        self.done_needed.store(true, Ordering::Release);
        group
    }

    /// Removes a [`WorkspaceGroup`]
    ///
    /// This function remove the `WorkspaceGroup` from the manager, then send `workspace_leave` and
    /// `output_leave` for its [`Workspace`] and [`Output`] respectively. The instance list is then
    /// drained to avoid further requests, and each instance is sent the `removed` event to notify
    /// the client.
    ///
    /// NOTE: It is the responsibility of the compositor to ensure there are no other instances of
    /// this specific [`WorkspaceGroup`] to avoid internal state and user data leaking. However,
    /// after calling this function the [`WorkspaceGroup`] will be functionally inert (it contains
    /// no client instances, and none can be added back).
    pub fn remove_group(&mut self, group: WorkspaceGroup) {
        self.groups.retain(|g| g != &group);

        let workspaces = group.workspaces();
        for workspace in workspaces {
            group.workspace_leave(&workspace);
        }

        let outputs = group.outputs();
        for output in outputs {
            group.output_leave(&output);
        }

        let mut inner = group.inner.0.lock().unwrap();

        for instance in inner.instances.drain(..) {
            instance.removed();
        }
        inner.done_needed.take();

        self.done_needed.store(true, Ordering::Release);
    }

    /// Gets an iterator over managed [`WorkspaceGroup`].
    pub fn groups(&self) -> std::slice::Iter<'_, WorkspaceGroup> {
        self.groups.iter()
    }
}

impl<D> Dispatch2<ExtWorkspaceGroupHandleV1, D> for WorkspaceGroupData
where
    D: WorkspaceHandler,
{
    fn request(
        &self,
        state: &mut D,
        client: &wayland_server::Client,
        resource: &ExtWorkspaceGroupHandleV1,
        request: <ExtWorkspaceGroupHandleV1 as wayland_server::Resource>::Request,
        _dhandle: &wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let Some(handle) = self.handle.upgrade() else {
            return;
        };

        let Some(_manager_resource) = self.manager.upgrade().ok() else {
            return;
        };

        let capabilities = handle.capabilities();

        use ext_workspace_group_handle_v1::Request;
        match request {
            Request::CreateWorkspace { workspace: _ }
                if capabilities.contains(GroupCapabilities::CreateWorkspace) =>
            {
                todo!();
            }
            Request::Destroy => {
                self.destroyed(state, client.id(), resource);
            }
            _ => {}
        }
    }

    fn destroyed(&self, _state: &mut D, _client: ClientId, resource: &ExtWorkspaceGroupHandleV1) {
        let Some(handle) = self.handle.upgrade() else {
            return;
        };

        let mut group = handle.inner.0.lock().unwrap();
        group.instances.retain(|i| i != resource);
    }
}

impl PartialEq for WorkspaceGroup {
    #[inline]
    fn eq(&self, other: &WorkspaceGroup) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for WorkspaceGroup {}

impl Hash for WorkspaceGroup {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.inner).hash(state);
    }
}

impl PartialEq for WeakWorkspaceGroup {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        std::sync::Weak::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for WeakWorkspaceGroup {}

impl Hash for WeakWorkspaceGroup {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::sync::Weak::as_ptr(&self.inner).hash(state)
    }
}

impl PartialEq<WeakWorkspaceGroup> for WorkspaceGroup {
    #[inline]
    fn eq(&self, other: &WeakWorkspaceGroup) -> bool {
        other.upgrade().map(|o| &o == self).unwrap_or(false)
    }
}

impl PartialEq<WorkspaceGroup> for WeakWorkspaceGroup {
    #[inline]
    fn eq(&self, other: &WorkspaceGroup) -> bool {
        self.upgrade().map(|o| &o == other).unwrap_or(false)
    }
}
