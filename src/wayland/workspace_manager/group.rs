//! Workspace Group

use std::{
    collections::HashSet,
    hash::Hash,
    sync::{Arc, Mutex},
};

use wayland_protocols::ext::workspace::v1::server::{
    ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1, GroupCapabilities},
    ext_workspace_handle_v1::ExtWorkspaceHandleV1,
    ext_workspace_manager_v1::ExtWorkspaceManagerV1,
};
use wayland_server::{
    Client, Dispatch, DisplayHandle, Resource, Weak, backend::ClientId, protocol::wl_output::WlOutput,
};

use crate::{
    output::{Output, WeakOutput},
    utils::user_data::UserDataMap,
    wayland::{
        Dispatch2,
        workspace_manager::{
            PendingRequest, WorkspaceHandler, WorkspaceManagerObjectData, WorkspaceManagerState,
            workspace::{Workspace, WorkspaceData},
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

/// State of a [`WorkspaceGroup`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGroupState {
    /// [`WorkspaceGroup`]'s advertized capabilities.
    pub capabilities: GroupCapabilities,
}

/// Workspace internal data.
#[derive(Debug)]
pub(crate) struct Inner {
    pending_state: Option<WorkspaceGroupState>,
    current_state: WorkspaceGroupState,

    pending_outputs: Option<HashSet<WeakOutput>>,
    outputs: HashSet<WeakOutput>,

    instances: Vec<ExtWorkspaceGroupHandleV1>,
    initial_done_sent: bool,
    pending_removal: bool,
}

/// Data associated with workspace group handles.
#[derive(Debug)]
pub struct WorkspaceGroupData {
    handle: WeakWorkspaceGroup,
    manager: Weak<ExtWorkspaceManagerV1>,
}

#[derive(Debug)]
pub(crate) enum PendingGroupRequest {
    CreateWorkspace(String),
}

impl WorkspaceGroup {
    fn new(capabilities: GroupCapabilities, capacity: usize) -> Self {
        let inner = Inner {
            pending_state: None,
            current_state: WorkspaceGroupState { capabilities },

            pending_outputs: None,
            outputs: HashSet::new(),

            instances: Vec::with_capacity(capacity),
            initial_done_sent: false,
            pending_removal: false,
        };

        Self {
            inner: Arc::new((Mutex::new(inner), UserDataMap::new())),
        }
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

    /// Access the [`WorkspaceGroup`] pending state.
    pub fn with_pending_state<F, T>(&self, with_state: F) -> T
    where
        F: FnOnce(&mut WorkspaceGroupState) -> T,
    {
        let mut inner = self.inner.0.lock().unwrap();

        if inner.pending_state.is_none() {
            inner.pending_state = Some(inner.current_state.clone());
        }

        with_state(inner.pending_state.as_mut().unwrap())
    }

    /// Access the [`WorkspaceGroup`] current state.
    pub fn with_current_state<F, T>(&self, with_state: F) -> T
    where
        F: FnOnce(&WorkspaceGroupState) -> T,
    {
        let inner = self.inner.0.lock().unwrap();
        with_state(&inner.current_state)
    }

    /// Add a workspace to this group.
    pub fn workspace_enter(&self, workspace: &Workspace) {
        workspace.assign(self);
    }

    /// Remove a workspace from this group.
    ///
    /// If the workspace wasn't in this group, this function does nothing.
    pub fn workspace_leave(&self, workspace: &Workspace) {
        workspace.leave(self);
    }

    /// Add an [`Output`] to this group.
    ///
    /// Outputs are stored as weak reference. As such, it is the responsibility of the caller to
    /// ensure [`WorkspaceGroup::output_leave`] is called, and the state is sent to clients before
    /// the [`Output`] is dropped.
    pub fn output_enter(&self, output: &Output) {
        self.with_pending_outputs(|outputs| outputs.insert(output.downgrade()));
    }

    /// Remove an [`Output`] from this group.
    ///
    /// Outputs are stored as weak reference. As such, it is the responsibility of the caller to
    /// ensure [`WorkspaceGroup::output_leave`] is called, and the state is sent to clients before
    /// the [`Output`] is dropped.
    pub fn output_leave(&self, output: &Output) {
        self.with_pending_outputs(|outputs| outputs.remove(&output.downgrade()));
    }

    /// Get an iterator over this group's pending [`Output`]s.
    pub fn pending_outputs(&self) -> impl Iterator<Item = Output> {
        let inner = self.inner.0.lock().unwrap();

        inner
            .pending_outputs
            .as_ref()
            .unwrap_or(&inner.outputs)
            .iter()
            .flat_map(WeakOutput::upgrade)
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// Get an iterator over this group's [`Output`]s.
    pub fn outputs(&self) -> impl Iterator<Item = Output> {
        let inner = self.inner.0.lock().unwrap();

        inner
            .outputs
            .iter()
            .flat_map(WeakOutput::upgrade)
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// Checks whether this group contains a given [`Output`].
    pub fn has_output(&self, output: &Output) -> bool {
        let inner = self.inner.0.lock().unwrap();

        inner.outputs.contains(&output.downgrade())
    }

    /// Remove the group.
    pub fn remove(&self) {
        self.inner.0.lock().unwrap().pending_removal = true;
    }

    /// Checks whether the [`WorkspaceGroup`] is pending removal.
    pub fn pending_removal(&self) -> bool {
        self.inner.0.lock().unwrap().pending_removal
    }

    /// Checks whether the [`WorkspaceGroup`] has pending state changes.
    ///
    /// This function returns whether there are pending state changes to send to clients. If the
    /// group is pending removal, state changes are ignored and `false` is returned instead.
    pub fn has_pending_changes(&self) -> bool {
        let inner = self.inner.0.lock().unwrap();

        if inner.pending_removal {
            return false;
        }

        !inner.initial_done_sent
            || inner
                .pending_state
                .as_ref()
                .is_some_and(|s| s.capabilities != inner.current_state.capabilities)
            || inner
                .pending_outputs
                .as_ref()
                .is_some_and(|o| o != &inner.outputs)
    }

    /// Send pending changes for this [`WorkspaceGroup`].
    ///
    /// This function send pending state changes to clients, with the exception of `workspace_enter`
    /// and `workspace_leave` that are handled by [`Workspace`]. Its purpose is to let compositor
    /// have fine control on the groups lifecycle. If the group is pending removal, no state changes
    /// will be sent and the function will return false.
    ///
    /// The function cannot remove groups from the [`WorkspaceManagerState`]. Compositor handling
    /// the lifecycle explicitly should call [`WorkspaceManagerState::send_removed_groups`] from
    /// time to time.
    pub fn send_pending_changes(&self) -> bool {
        let mut inner = self.inner.0.lock().unwrap();

        if inner.pending_removal {
            return false;
        }

        let changed = !inner.initial_done_sent;
        inner.initial_done_sent = true;

        changed | inner.send_pending_changes(self.downgrade())
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
        manager.workspace_group(&instance);

        let mut inner = self.inner.0.lock().unwrap();
        let state = &inner.current_state;

        instance.capabilities(state.capabilities);

        inner
            .outputs
            .iter()
            .flat_map(WeakOutput::upgrade)
            .for_each(|o| with_wl_output(&instance, &o, ExtWorkspaceGroupHandleV1::output_enter));

        inner.instances.push(instance);
    }

    pub(super) fn remove_instances(&self, manager: &ExtWorkspaceManagerV1) {
        let mut inner = self.inner.0.lock().unwrap();

        let manager_id = Resource::id(manager);
        inner
            .instances
            .extract_if(.., |i| {
                Resource::id(i).same_client_as(&manager_id)
                    && i.data::<WorkspaceGroupData>()
                        .is_some_and(|d| d.manager.id() == manager_id)
            })
            .for_each(std::mem::drop);
    }

    pub(crate) fn on_output_bind(&self, client: &Client, output: &WlOutput) {
        let inner = self.inner.0.lock().unwrap();

        for instance in inner
            .instances
            .iter()
            .filter(|i| i.client().as_ref() == Some(client))
        {
            instance.output_enter(output);

            if inner.initial_done_sent {
                let Some(manager) = instance
                    .data::<WorkspaceGroupData>()
                    .and_then(|d| d.manager.upgrade().ok())
                else {
                    continue;
                };

                manager.done();
            }
        }
    }

    pub(crate) fn send_workspace_enter(&self, workspace: &ExtWorkspaceHandleV1) {
        let inner = self.inner.0.lock().unwrap();

        let Some(w_manager_id) = workspace.data::<WorkspaceData>().map(|d| d.manager.id()) else {
            return;
        };

        for instance in &inner.instances {
            let Some(manager_id) = instance.data::<WorkspaceGroupData>().map(|d| d.manager.id()) else {
                continue;
            };

            if w_manager_id == manager_id {
                instance.workspace_enter(workspace);
            }
        }
    }

    pub(crate) fn send_workspace_leave(&self, workspace: &ExtWorkspaceHandleV1) {
        let inner = self.inner.0.lock().unwrap();

        let Some(w_manager_id) = workspace.data::<WorkspaceData>().map(|d| d.manager.id()) else {
            return;
        };

        for instance in &inner.instances {
            let Some(manager_id) = instance.data::<WorkspaceGroupData>().map(|d| d.manager.id()) else {
                continue;
            };

            if w_manager_id == manager_id {
                instance.workspace_leave(workspace);
            }
        }
    }

    fn with_pending_outputs<F, T>(&self, with_outputs: F) -> T
    where
        F: FnOnce(&mut HashSet<WeakOutput>) -> T,
    {
        let mut inner = self.inner.0.lock().unwrap();

        if inner.pending_outputs.is_none() {
            inner.pending_outputs = Some(inner.outputs.clone());
        }

        with_outputs(inner.pending_outputs.as_mut().unwrap())
    }
}

impl WeakWorkspaceGroup {
    /// Attempts to upgrade the `WeakWorkspaceGroup` to a [`WorkspaceGroup`]
    pub fn upgrade(&self) -> Option<WorkspaceGroup> {
        self.inner.upgrade().map(|inner| WorkspaceGroup { inner })
    }
}

fn with_wl_output<F>(instance: &ExtWorkspaceGroupHandleV1, output: &Output, process: F)
where
    F: Fn(&ExtWorkspaceGroupHandleV1, &WlOutput),
{
    let Some(client) = instance.client() else {
        return;
    };

    for wl_output in output.client_outputs(&client) {
        process(instance, &wl_output);
    }
}

impl Inner {
    fn send_pending_changes(&mut self, handle: WeakWorkspaceGroup) -> bool {
        let mut send_capabilities = false;

        if let Some(pending_state) = self.pending_state.take() {
            send_capabilities = pending_state.capabilities != self.current_state.capabilities;

            self.current_state = pending_state;
        }

        let mut old_outputs = Vec::new();
        let mut new_outputs = Vec::new();

        if let Some(pending_outputs) = self.pending_outputs.take() {
            old_outputs = self
                .outputs
                .difference(&pending_outputs)
                .flat_map(WeakOutput::upgrade)
                .collect();
            new_outputs = pending_outputs
                .difference(&self.outputs)
                .flat_map(WeakOutput::upgrade)
                .collect();

            self.outputs = pending_outputs.into_iter().filter(WeakOutput::is_alive).collect();
        } else {
            self.outputs.retain(WeakOutput::is_alive);
        }

        let changed = send_capabilities | !old_outputs.is_empty() | !new_outputs.is_empty();

        for output in &old_outputs {
            output.inner.0.lock().unwrap().workspace_groups.remove(&handle);
        }

        for output in &new_outputs {
            output
                .inner
                .0
                .lock()
                .unwrap()
                .workspace_groups
                .insert(handle.clone());
        }

        if changed {
            for instance in &self.instances {
                if send_capabilities {
                    instance.capabilities(self.current_state.capabilities);
                }

                old_outputs.iter().for_each(|o| {
                    with_wl_output(instance, o, ExtWorkspaceGroupHandleV1::output_leave);
                });

                new_outputs.iter().for_each(|o| {
                    with_wl_output(instance, o, ExtWorkspaceGroupHandleV1::output_enter);
                });
            }
        }

        changed
    }

    fn remove(&mut self, handle: WeakWorkspaceGroup) {
        let outputs: Vec<_> = self.outputs.drain().flat_map(|o| o.upgrade()).collect();

        for output in &outputs {
            output.inner.0.lock().unwrap().workspace_groups.remove(&handle);
        }

        for instance in std::mem::take(&mut self.instances) {
            outputs.iter().for_each(|o| {
                with_wl_output(&instance, o, ExtWorkspaceGroupHandleV1::output_leave);
            });
            instance.removed();
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
        let handle = WorkspaceGroup::new(capabilities, self.instances.len());

        for instance in &self.instances {
            let Ok(client) = self.display.get_client(instance.id()) else {
                continue;
            };

            handle.new_instance::<D>(&client, &self.display, instance);
        }

        self.groups.push(handle.clone());
        handle
    }

    /// Removes a [`WorkspaceGroup`]
    pub fn remove_group(&mut self, group: WorkspaceGroup) {
        group.remove();
    }

    /// Gets an iterator over managed [`WorkspaceGroup`].
    pub fn groups(&self) -> impl Iterator<Item = &WorkspaceGroup> {
        self.groups.iter()
    }

    /// Send pending changes for all [`WorkspaceGroup`].
    ///
    /// This function send pending group state changes and output events to clients, then return
    /// whether changes were sent. Groups marked for removal will be ignored when calling this
    /// function.
    ///
    /// The changes don't include `workspace_enter` and `workspace_leave` nor removal. Workspace
    /// event are handled by [`send_workspace_changes`], and removal is handled by
    /// [`send_removed_groups`].
    ///
    /// Calling this function directly isn't recommended and compositor should prefer calling
    /// [`send_pending_changes`]. Nevertheless, it's provided for compositor that want finer control
    /// on the ordering of operation (e.g. if you have group events from a protocol extension that
    /// should be sent alongside other group events).
    ///
    /// [`send_workspace_changes`]: WorkspaceManagerState::send_workspace_changes
    /// [`send_removed_groups`]: WorkspaceManagerState::send_removed_groups
    /// [`send_pending_changes`]: WorkspaceManagerState::send_pending_changes
    pub fn send_groups_changes(&mut self) -> bool {
        let mut changed = false;

        self.groups
            .iter_mut()
            .filter(|g| !g.pending_removal())
            .for_each(|g| {
                changed |= g.send_pending_changes();
            });

        changed
    }

    /// Notify clients that groups were removed.
    ///
    /// This function send `removed` events for groups pending removal, then remove them from the
    /// [`WorkspaceManagerState`].
    ///
    /// Calling this function directly isn't recommended and compositor should prefer calling
    /// [`WorkspaceManagerState::send_pending_changes`] instead. Nevertheless, it's provided for
    /// compositor that want finer control on the ordering of operation (e.g. if you have group
    /// events from a protocol extension that should be sent alongside other group events). When
    /// calling this function directly, it should be done after all workspace lifecycle updates.
    pub fn send_removed_groups(&mut self) -> bool {
        let mut changed = false;

        self.groups.retain_mut(|g| {
            let mut inner = g.inner.0.lock().unwrap();

            if !inner.pending_removal {
                true
            } else {
                inner.remove(g.downgrade());
                changed = true;
                false
            }
        });

        changed
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

        let Some(manager) = self.manager.upgrade().ok() else {
            return;
        };

        let capabilities = handle.with_current_state(|s| s.capabilities);

        use ext_workspace_group_handle_v1::Request;
        let pending = match request {
            Request::CreateWorkspace { workspace }
                if capabilities.contains(GroupCapabilities::CreateWorkspace) =>
            {
                WorkspaceHandler::create_workspace(state, resource, workspace.clone());
                PendingGroupRequest::CreateWorkspace(workspace.clone())
            }
            Request::Destroy => {
                self.destroyed(state, client.id(), resource);
                return;
            }
            _ => {
                return;
            }
        };

        WorkspaceManagerObjectData::add_pending(
            &manager,
            PendingRequest::Group(resource.downgrade(), pending),
        );
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &ExtWorkspaceGroupHandleV1) {
        if let Ok(manager) = self.manager.upgrade() {
            WorkspaceManagerObjectData::remove_pending(&manager, Resource::id(resource));
        }

        WorkspaceHandler::group_destroyed(state, resource);

        let Some(handle) = self.handle.upgrade() else {
            return;
        };

        let mut group = handle.inner.0.lock().unwrap();

        if let Some(pos) = group.instances.iter().position(|i| i.id() == resource.id()) {
            group.instances.remove(pos);
        }
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
