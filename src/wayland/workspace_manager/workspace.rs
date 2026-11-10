//! Workspace

use std::{
    hash::Hash,
    sync::{Arc, Mutex},
};

use wayland_protocols::ext::workspace::v1::server::{
    ext_workspace_handle_v1::{ExtWorkspaceHandleV1, State, WorkspaceCapabilities},
    ext_workspace_manager_v1::ExtWorkspaceManagerV1,
};
use wayland_server::{Client, Dispatch, DisplayHandle, Resource, Weak};

use crate::{
    utils::user_data::UserDataMap,
    wayland::workspace_manager::{
        WorkspaceManagerState,
        group::{WeakWorkspaceGroup, WorkspaceGroup},
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

/// State of a [`Workspace`].
#[derive(Debug, Clone)]
pub struct WorkspaceState {
    /// [`Workspace`]'s name.
    pub name: String,
    /// [`Workspace`]'s unique identifier.
    pub id: Option<String>,
    /// [`Workspace`]'s coordinates.
    pub coordinates: Vec<u32>,
    /// [`Workspace`]'s state.
    pub state: State,
    /// [`Workspace`]'s advertized capabilities.
    pub capabilities: WorkspaceCapabilities,
}

/// Workspace internal data.
#[derive(Debug)]
pub(crate) struct Inner {
    pending_state: Option<WorkspaceState>,
    current_state: WorkspaceState,

    pending_group: Option<WeakWorkspaceGroup>,
    group: Option<WeakWorkspaceGroup>,

    instances: Vec<ExtWorkspaceHandleV1>,
    initial_done_sent: bool,
    pending_removal: bool,
}

/// Data associated with workspace handles.
#[derive(Debug)]
pub struct WorkspaceData {
    handle: WeakWorkspace,
    pub(crate) manager: Weak<ExtWorkspaceManagerV1>,
}

/// Workspace Builder
#[derive(Debug)]
pub struct Builder {
    workspace_state: WorkspaceState,
}

impl Workspace {
    /// Gets a builder object for [`Workspace`].
    pub fn builder(name: String) -> Builder {
        Builder {
            workspace_state: WorkspaceState {
                name,
                id: None,
                coordinates: Vec::new(),
                state: State::empty(),
                capabilities: WorkspaceCapabilities::all(),
            },
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

    /// Access the workspace pending state.
    pub fn with_pending_state<F, T>(&self, with_state: F) -> T
    where
        F: FnOnce(&mut WorkspaceState) -> T,
    {
        let mut inner = self.inner.0.lock().unwrap();

        if inner.pending_state.is_none() {
            inner.pending_state = Some(inner.current_state.clone());
        }

        with_state(inner.pending_state.as_mut().unwrap())
    }

    /// Access the workspace current state.
    pub fn with_current_state<F, T>(&self, with_state: F) -> T
    where
        F: FnOnce(&WorkspaceState) -> T,
    {
        let inner = self.inner.0.lock().unwrap();
        with_state(&inner.current_state)
    }

    /// Get a handle to the workspace's pending group.
    pub fn pending_group(&self) -> Option<WorkspaceGroup> {
        let inner = self.inner.0.lock().unwrap();

        inner.pending_group.as_ref().and_then(WeakWorkspaceGroup::upgrade)
    }

    /// Get a handle to the workspace group.
    pub fn group(&self) -> Option<WorkspaceGroup> {
        let inner = self.inner.0.lock().unwrap();

        inner.group.as_ref().and_then(WeakWorkspaceGroup::upgrade)
    }

    /// Activate the workspace.
    pub fn activate(&self) {
        self.with_pending_state(|s| s.state.set(State::Active, true));
    }

    /// Deactivate the workspace.
    pub fn deactivate(&self) {
        self.with_pending_state(|s| s.state.set(State::Active, false));
    }

    /// Assign the workspace to a group.
    pub fn assign(&self, group: &WorkspaceGroup) {
        let mut inner = self.inner.0.lock().unwrap();

        inner.pending_group = Some(group.downgrade());
    }

    /// Unassign the workspace from a group.
    pub fn leave(&self, group: &WorkspaceGroup) {
        let mut inner = self.inner.0.lock().unwrap();

        // Either pending_group == group, or group is stale and shouldn't be accounted for.
        inner.pending_group.take_if(|g| g == group);
    }

    /// Remove the workspace.
    pub fn remove(&self) {
        let mut inner = self.inner.0.lock().unwrap();

        inner.pending_removal = true;
    }

    /// Checks whether the [`Workspace`] is pending removal.
    pub fn pending_removal(&self) -> bool {
        self.inner.0.lock().unwrap().pending_removal
    }

    /// Checks whether the [`Workspace`] has pending state changes.
    ///
    /// This function returns whether there are pending state changes to send to clients. If the
    /// workspace is pending removal, state changes are ignored and `false` is returned instead.
    pub fn has_pending_changes(&self) -> bool {
        let inner = self.inner.0.lock().unwrap();

        if inner.pending_removal {
            return false;
        }

        !inner.initial_done_sent
            || inner.pending_group != inner.group
            || inner.pending_state.as_ref().is_some_and(|s| {
                s.name != inner.current_state.name
                    || (s.id.is_some() && inner.current_state.id.is_none())
                    || s.coordinates != inner.current_state.coordinates
                    || s.state != inner.current_state.state
                    || s.capabilities != inner.current_state.capabilities
            })
    }

    /// Send pending changes for this [`Workspace`].
    ///
    /// This function send pending state changes as well as workspace_enter and workspace_leave
    /// event for a specific. Its purpose is to let compositor have fine control the workspace
    /// lifecycle. If the workspace is pending removal, no state changes will be sent, and this
    /// function will return false.
    ///
    /// The function cannot remove workspaces from the [`WorkspaceManagerState`]. When handling the
    /// lifecycle explicitly, compositor should call [`Workspace::send_pending_changes`] before
    /// [`WorkspaceManagerState::send_removed_groups`].
    pub fn send_pending_changes(&self) -> bool {
        let mut inner = self.inner.0.lock().unwrap();

        if inner.pending_removal {
            return false;
        }

        let changed = !inner.initial_done_sent;
        inner.initial_done_sent = true;

        changed | inner.send_pending_changes()
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
        manager.workspace(&instance);

        let mut inner = self.inner.0.lock().unwrap();
        let state = &inner.current_state;

        instance.name(state.name.clone());
        if let Some(id) = state.id.clone() {
            instance.id(id);
        }

        if !state.coordinates.is_empty() {
            instance.coordinates(coordinate_to_wayland(&state.coordinates));
        }

        instance.state(state.state);
        instance.capabilities(state.capabilities);

        if let Some(group) = inner.group.as_ref().and_then(WeakWorkspaceGroup::upgrade) {
            group.send_workspace_enter(&instance);
        }

        inner.instances.push(instance);
    }
}

fn coordinate_to_wayland(coordinates: &[u32]) -> Vec<u8> {
    coordinates.iter().copied().flat_map(u32::to_ne_bytes).collect()
}

impl WeakWorkspace {
    /// Attempts to upgrade the `WeakWorkspace` to a [`Workspace`]
    pub fn upgrade(&self) -> Option<Workspace> {
        self.inner.upgrade().map(|inner| Workspace { inner })
    }
}

impl Inner {
    fn send_pending_changes(&mut self) -> bool {
        let mut send_name = false;
        let mut send_id = false;
        let mut new_coord = None;
        let mut send_caps = false;
        let mut send_state = false;

        if let Some(mut pending_state) = self.pending_state.take() {
            if self
                .current_state
                .id
                .as_ref()
                .is_some_and(|id| Some(id) != pending_state.id.as_ref())
            {
                tracing::error!("Protocol disallow changing workspace ID once set.");
                pending_state.id = self.current_state.id.clone();
            };

            send_name = self.current_state.name != pending_state.name;
            send_id = self.current_state.id != pending_state.id;

            new_coord = (self.current_state.coordinates != pending_state.coordinates)
                .then(|| coordinate_to_wayland(&pending_state.coordinates));

            send_caps = self.current_state.capabilities != pending_state.capabilities;
            send_state = self.current_state.state != pending_state.state;

            self.current_state = pending_state;
        };

        let mut old_group = None;
        let mut new_group = None;

        if self.pending_group != self.group {
            old_group = self.group.as_ref().and_then(WeakWorkspaceGroup::upgrade);

            let pending_group = self.pending_group.as_ref().and_then(WeakWorkspaceGroup::upgrade);
            if let Some(group) = pending_group
                && !group.pending_removal()
            {
                new_group = Some(group);
            } else {
                self.pending_group = None;
            }

            self.group = self.pending_group.clone();
        } else if let Some(group) = self.group.as_ref().and_then(WeakWorkspaceGroup::upgrade)
            && group.pending_removal()
        {
            self.group.take();
            old_group = Some(group);
        }

        let changed = send_name
            | send_id
            | new_coord.is_some()
            | send_caps
            | send_state
            | old_group.is_some()
            | new_group.is_some();

        if changed {
            for instance in self.instances.iter() {
                if send_name {
                    instance.name(self.current_state.name.clone());
                }

                if send_id {
                    instance.id(self.current_state.id.clone().unwrap());
                }

                if let Some(v) = new_coord.clone() {
                    instance.coordinates(v);
                }

                if send_caps {
                    instance.capabilities(self.current_state.capabilities);
                }

                if send_state {
                    instance.state(self.current_state.state);
                }

                if let Some(group) = old_group.as_ref() {
                    group.send_workspace_leave(instance);
                }

                if let Some(group) = new_group.as_ref() {
                    group.send_workspace_enter(instance);
                }
            }
        }

        changed
    }

    fn remove(&mut self) {
        for instance in std::mem::take(&mut self.instances) {
            if let Some(group) = self.group.as_ref().and_then(WeakWorkspaceGroup::upgrade) {
                group.send_workspace_leave(&instance);
            }

            instance.removed();
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

    /// Creates a new workspace with the state defined in [`Builder`].
    pub fn new_workspace_with_builder<D>(&mut self, builder: Builder) -> Workspace
    where
        D: Dispatch<ExtWorkspaceHandleV1, WorkspaceData>,
        D: 'static,
    {
        let handle = builder.build(self.instances.len());

        for instance in &self.instances {
            let Ok(client) = self.display.get_client(instance.id()) else {
                continue;
            };

            handle.new_instance::<D>(&client, &self.display, instance);
        }

        self.workspaces.push(handle.clone());
        handle
    }

    /// Removes a [`Workspace`].
    pub fn remove_workspace(&mut self, workspace: Workspace) {
        workspace.remove();
    }

    /// Gets an iterator over managed [`Workspace`]
    pub fn workspaces(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces.iter()
    }

    /// Gets an iterator over the workspace's group.
    pub fn group_workspaces(&self, group: &WorkspaceGroup) -> impl Iterator<Item = &Workspace> {
        self.workspaces.iter().filter({
            let group = group.downgrade();
            move |w| {
                let inner = w.inner.0.lock().unwrap();
                inner.group.as_ref().is_some_and(|g| g == &group)
            }
        })
    }

    /// Send pending changes for all [`Workspace`]
    ///
    /// This function send pending state changes as well as workspace_enter and workspace_leave
    /// event. If a workspace was marked for removal, it'll be removed from the
    /// [`WorkspaceManagerState`] once that function return. For removed workspaces, only
    /// `workspace_leave` and `remove` will be sent to clients.
    ///
    /// Calling this function directly isn't recommended and compositor should prefer calling
    /// [`send_pending_changes`]. Nevertheless, it's provided for compositor that want finer control
    /// on the ordering of operation (e.g. to send workspace event for a protocol extension that
    /// should be sent alongside other events.). When calling this function directly, it should be
    /// done before the call to [`send_removed_groups`].
    ///
    /// [`send_pending_changes`]: WorkspaceManagerState::send_pending_changes
    /// [`send_removed_groups`]: WorkspaceManagerState::send_removed_groups
    pub fn send_workspace_changes(&mut self) -> bool {
        let mut changed = false;

        self.workspaces.retain_mut(|w| {
            if w.pending_removal() {
                changed = true;
                w.inner.0.lock().unwrap().remove();
                return false;
            }

            changed |= w.send_pending_changes();

            true
        });

        changed
    }

    /// Notify clients that workspaces were removed.
    ///
    /// This function send `removed` events for workspace pending removal, then remove them from the
    /// [`WorkspaceManagerState`]. In general, compositor should prefer [`send_workspace_changes`]
    /// or [`send_pending_changes`]. This function is exposed for compositor that require finer
    /// control on the lifecycle. Because it might send `workspace_leave`, this function should be
    /// called before [`send_removed_groups`].
    ///
    /// [`send_workspace_changes`]: WorkspaceManagerState::send_workspace_changes
    /// [`send_pending_changes`]: WorkspaceManagerState::send_pending_changes
    /// [`send_removed_groups`]: WorkspaceManagerState::send_removed_groups
    pub fn send_removed_workspaces(&mut self) -> bool {
        let mut changed = false;

        self.workspaces.retain_mut(|w| {
            let mut inner = w.inner.0.lock().unwrap();

            if inner.pending_removal {
                changed = true;
                inner.remove();
                return false;
            }

            true
        });

        changed
    }
}

impl Builder {
    /// Set the workspace unique identifier.
    pub fn with_id(mut self, id: String) -> Self {
        self.workspace_state.id = Some(id);
        self
    }

    /// Set the workspace initial coordinates.
    pub fn with_coordinates(mut self, coordinates: impl IntoIterator<Item = u32>) -> Self {
        self.workspace_state.coordinates = coordinates.into_iter().collect();
        self
    }

    /// Set the workspace initial state.
    pub fn with_state(mut self, state: State) -> Self {
        self.workspace_state.state = state;
        self
    }

    /// Set the workspace initial capabilities.
    pub fn with_capabilities(mut self, capabilities: WorkspaceCapabilities) -> Self {
        self.workspace_state.capabilities = capabilities;
        self
    }

    fn build(self, initial_capacity: usize) -> Workspace {
        let inner = Inner {
            pending_state: None,
            current_state: self.workspace_state,

            pending_group: None,
            group: None,

            instances: Vec::with_capacity(initial_capacity),
            initial_done_sent: false,
            pending_removal: false,
        };

        Workspace {
            inner: Arc::new((Mutex::new(inner), UserDataMap::new())),
        }
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
