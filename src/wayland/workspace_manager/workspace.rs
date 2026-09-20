//! Workspace

use std::{
    hash::Hash,
    sync::{Arc, Mutex, atomic::Ordering},
};

use portable_atomic::AtomicBool;
use wayland_protocols::ext::workspace::v1::server::{
    ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
    ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1, State, WorkspaceCapabilities},
    ext_workspace_manager_v1::ExtWorkspaceManagerV1,
};
use wayland_server::{
    Client, Dispatch, DisplayHandle, Resource, Weak,
    backend::{ClientId, ObjectId},
};

use crate::{
    utils::user_data::UserDataMap,
    wayland::{
        Dispatch2,
        workspace_manager::{
            PendingRequest, WorkspaceHandler, WorkspaceManagerObjectData, WorkspaceManagerState,
            group::{WeakWorkspaceGroup, WorkspaceGroup},
        },
    },
};

const WORKSPACE_VERSION: u32 = 1;

/// Handle to a Workspace.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub(crate) inner: Arc<(Mutex<Inner>, UserDataMap)>,
}

/// Weak version of [`Workspace`].
#[derive(Debug, Clone)]
pub struct WeakWorkspace {
    inner: std::sync::Weak<(Mutex<Inner>, UserDataMap)>,
}

/// Workspace internal data.
#[derive(Debug)]
pub(crate) struct Inner {
    id: Option<String>,
    name: String,
    coordinates: Vec<u32>,
    state: State,
    capabilities: WorkspaceCapabilities,

    pub(crate) group: Option<WeakWorkspaceGroup>,

    pub(crate) instances: Vec<ExtWorkspaceHandleV1>,
    done_needed: Option<Arc<AtomicBool>>,
}

/// Data associated with workspace handles.
#[derive(Debug)]
pub struct WorkspaceData {
    handle: WeakWorkspace,
    manager: Weak<ExtWorkspaceManagerV1>,
}

/// Workspace Builder
#[derive(Debug)]
pub struct Builder {
    id: Option<String>,
    name: String,
    coordinates: Vec<u32>,
    state: State,
    capabilities: WorkspaceCapabilities,
}

#[derive(Debug)]
pub(crate) enum PendingWorkspaceRequest {
    Activate,
    Deactivate,
    Assign(Weak<ExtWorkspaceGroupHandleV1>),
    Remove,
}

impl Workspace {
    /// Gets a builder object for [`Workspace`].
    pub fn builder(name: String) -> Builder {
        Builder {
            id: None,
            name,
            coordinates: Default::default(),
            state: State::empty(),
            capabilities: WorkspaceCapabilities::all(),
        }
    }

    /// Gets the `Workspace` associated with an [`ExtWorkspaceHandleV1`].
    pub fn from_resource(resource: &ExtWorkspaceHandleV1) -> Option<Self> {
        resource.data::<WorkspaceData>().and_then(|d| d.handle.upgrade())
    }

    /// Creates a new [`WeakWorkspace`] pointing to the same Workspace.
    pub fn downgrade(&self) -> WeakWorkspace {
        WeakWorkspace {
            inner: Arc::downgrade(&self.inner),
        }
    }

    /// Access the [`UserDataMap`] associated with this [`Workspace`]
    pub fn user_data(&self) -> &UserDataMap {
        &self.inner.1
    }

    /// Gets the current [`State`] of the `Workspace`.
    pub fn state(&self) -> State {
        self.inner.0.lock().unwrap().state
    }

    /// Sets the current [`State`] of the `Workspace`.
    pub fn set_state(&self, new_state: State) {
        self.inner.0.lock().unwrap().set_state(new_state);
    }

    /// Checks whether the `Workspace` is active.
    pub fn active(&self) -> bool {
        self.state().contains(State::Active)
    }

    /// Activate the workspace.
    ///
    /// This function is a shorthand to calling [`Workspace::set_state`] when only the active flag
    /// has to be set.
    pub fn activate(&mut self) {
        self.inner.0.lock().unwrap().set_state_flag(State::Active, true);
    }

    /// Activate the workspace.
    ///
    /// This function is a shorthand to calling [`Workspace::set_state`] when only the active flag
    /// has to be unset.
    pub fn deactivate(&mut self) {
        self.inner.0.lock().unwrap().set_state_flag(State::Active, false);
    }

    /// Checks whether the `Workspace`'s urgent flag is set.
    pub fn urgent(&self) -> bool {
        self.state().contains(State::Urgent)
    }

    /// Sets the `Workspace`'s urgent flag.
    ///
    /// This function is a shorthand to calling [`Workspace::set_state`] when only the Urgent flag
    /// need to be changed.
    pub fn set_urgent(&mut self, urgent: bool) {
        self.inner.0.lock().unwrap().set_state_flag(State::Urgent, urgent);
    }

    /// Checks whether the `Workspace`'s hidden flag is set.
    pub fn hidden(&self) -> bool {
        self.state().contains(State::Hidden)
    }

    /// Sets the `Workspace`'s hidden flag.
    ///
    /// This function is a shorthand to calling [`Workspace::set_state`] when only the hidden flag
    /// need to be changed.
    pub fn set_hidden(&mut self, hidden: bool) {
        self.inner.0.lock().unwrap().set_state_flag(State::Hidden, hidden);
    }

    /// Gets the `Workspace`'s id.
    pub fn id(&self) -> Option<String> {
        self.inner.0.lock().unwrap().id.clone()
    }

    /// Sets the `Workspace`'s id.
    ///
    /// `Workspace` can be assigned a unique, stable identifier. If the id was set, this function
    /// returns true.
    ///
    /// NOTE: It's up to the compositor to ensure the id are unique. Not doing that is a protocol
    /// violation.
    pub fn set_id(&mut self, id: String) -> bool {
        self.inner.0.lock().unwrap().set_id(id)
    }

    /// Gets the `Workspace`'s name.
    pub fn name(&self) -> String {
        self.inner.0.lock().unwrap().name.clone()
    }

    /// Sets the `Workspace`'s name.
    pub fn set_name(&mut self, name: String) {
        self.inner.0.lock().unwrap().set_name(name);
    }

    /// Gets the `Workspace`'s coordinates.
    pub fn coordinates(&self) -> Vec<u32> {
        self.inner.0.lock().unwrap().coordinates.clone()
    }

    /// Sets the `Workspace`'s coordinates.
    pub fn set_coordinates(&self, coordinates: Vec<u32>) {
        self.inner.0.lock().unwrap().set_coordinates(coordinates);
    }

    /// Gets the `Workspace`'s capabilities.
    pub fn capabilities(&self) -> WorkspaceCapabilities {
        self.inner.0.lock().unwrap().capabilities
    }

    /// Sets the `Workspace`'s capabilities.
    pub fn set_capabilities(&mut self, capabilities: WorkspaceCapabilities) {
        self.inner.0.lock().unwrap().set_capabilities(capabilities);
    }

    /// Gets the `Workspace`'s group, if any.
    pub fn group(&self) -> Option<WorkspaceGroup> {
        self.inner
            .0
            .lock()
            .unwrap()
            .group
            .as_ref()
            .and_then(WeakWorkspaceGroup::upgrade)
    }

    pub(super) fn new_instance<D>(
        &self,
        client: &Client,
        display: &DisplayHandle,
        manager: &ExtWorkspaceManagerV1,
    ) where
        D: Dispatch<ExtWorkspaceHandleV1, WorkspaceData>,
        D: 'static,
    {
        let Ok(instance) = client.create_resource::<ExtWorkspaceHandleV1, _, D>(
            display,
            WORKSPACE_VERSION,
            WorkspaceData {
                handle: self.downgrade(),
                manager: manager.downgrade(),
            },
        ) else {
            return;
        };

        let mut inner = self.inner.0.lock().unwrap();

        instance.name(inner.name.clone());
        if let Some(id) = inner.id.clone() {
            instance.id(id);
        }

        instance.state(inner.state);
        instance.capabilities(inner.capabilities);

        if !inner.coordinates.is_empty() {
            let coord: Vec<u8> = inner
                .coordinates
                .iter()
                .copied()
                .flat_map(u32::to_ne_bytes)
                .collect();

            instance.coordinates(coord);
        }

        inner.instances.push(instance);
    }

    pub(super) fn remove_instance(&self, manager: &ExtWorkspaceManagerV1, with_events: bool) {
        let mut inner = self.inner.0.lock().unwrap();

        let manager_id = Resource::id(manager);
        let extracted = inner.instances.extract_if(.., |i| {
            Resource::id(i).same_client_as(&manager_id)
                && i.data::<WorkspaceData>().unwrap().manager.id() == manager_id
        });

        if with_events {
            for instance in extracted {
                instance.removed();
            }
        } else {
            extracted.for_each(std::mem::drop);
        }
    }
}

impl WeakWorkspace {
    /// Attempts to upgrade the `WeakWorkspace` to a [`Workspace`]
    pub fn upgrade(&self) -> Option<Workspace> {
        self.inner.upgrade().map(|inner| Workspace { inner })
    }
}

impl Inner {
    pub(crate) fn instance_with_manager(&self, manager: &ObjectId) -> Option<&ExtWorkspaceHandleV1> {
        self.instances.iter().find(|i| {
            Resource::id(*i).same_client_as(manager)
                && &i.data::<WorkspaceData>().unwrap().manager.id() == manager
        })
    }

    fn set_name(&mut self, name: String) {
        if self.name != name {
            self.name = name;

            for instance in &self.instances {
                instance.name(self.name.clone());
            }

            self.need_done()
        }
    }

    fn set_id(&mut self, id: String) -> bool {
        if self.id.is_none() {
            self.id = Some(id.clone());

            for instance in &self.instances {
                instance.id(id.clone());
            }

            self.need_done();
            true
        } else {
            false
        }
    }

    fn set_coordinates(&mut self, coordinates: Vec<u32>) {
        if self.coordinates != coordinates {
            self.coordinates = coordinates;

            let coord: Vec<u8> = self
                .coordinates
                .iter()
                .copied()
                .flat_map(u32::to_ne_bytes)
                .collect();

            for instance in &self.instances {
                instance.coordinates(coord.clone());
            }
        }
    }

    fn set_state(&mut self, new_state: State) {
        if self.state != new_state {
            self.state = new_state;

            self.send_state();
        }
    }

    fn set_state_flag(&mut self, flag: State, value: bool) {
        if self.state.contains(flag) != value {
            self.state.toggle(flag);

            self.send_state();
        }
    }

    fn set_capabilities(&mut self, capabilities: WorkspaceCapabilities) {
        if self.capabilities != capabilities {
            self.capabilities = capabilities;

            for instance in &self.instances {
                instance.capabilities(capabilities);
            }

            self.need_done();
        }
    }

    #[inline]
    fn send_state(&mut self) {
        for instance in &self.instances {
            instance.state(self.state);
        }

        self.need_done()
    }

    #[inline]
    fn need_done(&mut self) {
        if let Some(done_needed) = self.done_needed.as_mut() {
            done_needed.store(true, Ordering::Release);
        }
    }
}

impl WorkspaceManagerState {
    /// Creates a new workspace with default state.
    pub fn new_workspace<D>(&mut self, name: String) -> Workspace
    where
        D: Dispatch<ExtWorkspaceHandleV1, WorkspaceData>,
        D: 'static,
    {
        self.new_workspace_with_builder::<D>(Workspace::builder(name))
    }

    /// Creates a new workspace with default state.
    pub fn new_workspace_with_builder<D>(&mut self, builder: Builder) -> Workspace
    where
        D: Dispatch<ExtWorkspaceHandleV1, WorkspaceData>,
        D: 'static,
    {
        let handle = builder.build(self.instances.len(), self.done_needed.clone());

        for instance in &self.instances {
            let Ok(client) = self.display.get_client(instance.id()) else {
                continue;
            };

            handle.new_instance::<D>(&client, &self.display, instance);
        }

        self.workspaces.push(handle.clone());
        self.done_needed.store(true, Ordering::Release);
        handle
    }

    /// Removes a [`Workspace`].
    ///
    /// This function remove the `Workspace` from the manager, then send workspace_leave event on its active
    /// [`WorkspaceGroup`] if any. The instance list is then drained to avoid further requests, and
    /// each instance is sent the `removed` event to notify the client.
    ///
    /// NOTE: It is the responsibility of the compositor to ensure there are no other instance of
    /// this specific [`Workspace`] to avoid the internal state and user_data leaking. However,
    /// after calling this function, the [`Workspace`] object will be functionally inert (it has no
    /// client instances, and none can be added back).
    pub fn remove_workspace(&mut self, workspace: Workspace) {
        self.workspaces.retain(|w| w != &workspace);

        if let Some(group) = workspace.group() {
            group.workspace_leave(&workspace);
        }

        let mut inner = workspace.inner.0.lock().unwrap();

        for instance in inner.instances.drain(..) {
            instance.removed();
        }

        inner.done_needed.take();

        self.done_needed.store(true, Ordering::Release);
    }

    /// Gets an iterator over managed [`Workspace`]
    pub fn workspaces(&self) -> std::slice::Iter<'_, Workspace> {
        self.workspaces.iter()
    }
}

impl<D> Dispatch2<ExtWorkspaceHandleV1, D> for WorkspaceData
where
    D: WorkspaceHandler,
{
    fn request(
        &self,
        state: &mut D,
        client: &wayland_server::Client,
        resource: &ExtWorkspaceHandleV1,
        request: <ExtWorkspaceHandleV1 as Resource>::Request,
        _dhandle: &wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let Some(handle) = self.handle.upgrade() else {
            return;
        };

        let Some(manager) = self.manager.upgrade().ok() else {
            return;
        };

        let capabilities = handle.capabilities();

        use ext_workspace_handle_v1::Request;
        let pending = match request {
            Request::Destroy => {
                self.destroyed(state, client.id(), resource);
                return;
            }
            Request::Activate if capabilities.contains(WorkspaceCapabilities::Activate) => {
                WorkspaceHandler::activate(state, resource);
                PendingWorkspaceRequest::Activate
            }
            Request::Deactivate if capabilities.contains(WorkspaceCapabilities::Deactivate) => {
                WorkspaceHandler::deactivate(state, resource);
                PendingWorkspaceRequest::Deactivate
            }
            Request::Assign { workspace_group } if capabilities.contains(WorkspaceCapabilities::Assign) => {
                WorkspaceHandler::assign(state, resource, &workspace_group);
                PendingWorkspaceRequest::Assign(workspace_group.downgrade())
            }
            Request::Remove if capabilities.contains(WorkspaceCapabilities::Remove) => {
                WorkspaceHandler::remove(state, resource);
                PendingWorkspaceRequest::Remove
            }
            _ => {
                return;
            }
        };

        WorkspaceManagerObjectData::add_pending(
            &manager,
            PendingRequest::Workspace(resource.downgrade(), pending),
        );
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, resource: &ExtWorkspaceHandleV1) {
        let Some(handle) = self.handle.upgrade() else {
            return;
        };

        let mut workspace = handle.inner.0.lock().unwrap();

        if let Ok(manager) = self.manager.upgrade() {
            WorkspaceManagerObjectData::remove_pending(&manager, Resource::id(resource));
        }

        if let Some(pos) = workspace
            .instances
            .iter()
            .position(|w| Resource::id(w) == Resource::id(resource))
        {
            let instance = workspace.instances.remove(pos);
            WorkspaceHandler::workspace_destroyed(state, &instance);
        }
    }
}

impl Builder {
    /// Sets the id to use on [`Workspace`] creation.
    ///
    /// NOTE: Workspace id are supposed to be unique. It is the responsibility of the compositor to
    /// ensure this holds true at all time.
    pub fn with_id(self, id: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
            ..self
        }
    }

    /// Sets the coordinates to use on [`Workspace`] creation.
    pub fn with_coordinates(self, coordinates: impl Into<Vec<u32>>) -> Self {
        Self {
            coordinates: coordinates.into(),
            ..self
        }
    }

    /// Sets the [`Workspace`] initial state.
    pub fn with_state(self, state: State) -> Self {
        Self { state, ..self }
    }

    /// Sets the [`Workspace`] initial capabilities.
    pub fn with_capabilities(self, capabilities: WorkspaceCapabilities) -> Self {
        Self { capabilities, ..self }
    }

    fn build(self, capacity: usize, done_needed: Arc<AtomicBool>) -> Workspace {
        let Self {
            id,
            name,
            coordinates,
            state,
            capabilities,
        } = self;
        let inner = Arc::new((
            Mutex::new(Inner {
                id,
                name,
                coordinates,
                state,
                capabilities,
                group: None,
                instances: Vec::with_capacity(capacity),
                done_needed: Some(done_needed),
            }),
            UserDataMap::default(),
        ));

        Workspace { inner }
    }
}

impl PartialEq for Workspace {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for Workspace {}

impl Hash for Workspace {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.inner).hash(state);
    }
}

impl PartialEq for WeakWorkspace {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        std::sync::Weak::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for WeakWorkspace {}

impl Hash for WeakWorkspace {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::sync::Weak::as_ptr(&self.inner).hash(state);
    }
}

impl PartialEq<WeakWorkspace> for Workspace {
    #[inline]
    fn eq(&self, other: &WeakWorkspace) -> bool {
        other.upgrade().map(|o| &o == self).unwrap_or(false)
    }
}

impl PartialEq<Workspace> for WeakWorkspace {
    #[inline]
    fn eq(&self, other: &Workspace) -> bool {
        self.upgrade().map(|o| &o == other).unwrap_or(false)
    }
}
