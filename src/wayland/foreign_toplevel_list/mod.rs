//! Foreign toplevel list
//!
//! The purpose of this protocol is to provide protocol object handles for toplevels.
//!
//! ```no_run
//! use smithay::wayland::foreign_toplevel_list::{ForeignToplevelListState, ForeignToplevelListHandler};
//!
//! pub struct State {
//!     foreign_toplevel_list: ForeignToplevelListState,
//! }
//!
//! smithay::delegate_foreign_toplevel_list!(State);
//!
//! impl ForeignToplevelListHandler for State {
//!     fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
//!         &mut self.foreign_toplevel_list
//!     }
//! }
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//! let mut state = State {
//!     foreign_toplevel_list: ForeignToplevelListState::new::<State>(&display_handle),
//! };
//!
//! let handle = state.foreign_toplevel_list.new_toplevel::<State>("Window Title", "com.example");
//!
//! // Handle can be used to update title and app_id
//! handle.send_title("Window title has changed");
//! handle.send_done();
//!
//! // Handle can also be used to close the window, after this call the handle will become inert,
//! // and the handle will no longer be announced to clients
//! handle.send_closed();
//! ```

use std::sync::{Arc, Mutex};

use rand::distr::{Alphanumeric, SampleString};
use wayland_protocols::ext::foreign_toplevel_list::v1::server::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};
use wayland_server::{
    backend::{ClientId, GlobalId},
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak,
};

use crate::utils::user_data::UserDataMap;

/// Handler for foreign toplevel list protocol
pub trait ForeignToplevelListHandler:
    GlobalDispatch<ExtForeignToplevelListV1, ForeignToplevelListGlobalData>
    + Dispatch<ExtForeignToplevelListV1, ()>
    + Dispatch<ExtForeignToplevelHandleV1, ForeignToplevelHandle>
    + 'static
{
    /// [ForeignToplevelListState] getter
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState;
}

#[derive(Debug)]
struct ForeignToplevelHandleInner {
    identifier: String,
    title: String,
    app_id: String,
    // Each ExtForeignToplevelHandleV1 contains the handle in it's user data,
    // so this ref has to be weak
    instances: Vec<Weak<ExtForeignToplevelHandleV1>>,
    closed: bool,
}

impl ForeignToplevelHandleInner {
    /// The toplevel has been closed
    fn send_closed(&mut self) {
        if self.closed {
            return;
        }

        self.closed = true;
        // drain to prevent any events from being sent to closed handles
        for toplevel in self.instances.drain(..) {
            if let Ok(toplevel) = toplevel.upgrade() {
                toplevel.closed();
            }
        }
    }
}

impl Drop for ForeignToplevelHandleInner {
    fn drop(&mut self) {
        self.send_closed()
    }
}

/// Weak version of [ForeignToplevelHandle]
#[derive(Debug, Clone)]
pub struct ForeignToplevelWeakHandle {
    inner: std::sync::Weak<(Mutex<ForeignToplevelHandleInner>, UserDataMap)>,
}

impl ForeignToplevelWeakHandle {
    /// Upgrade weak [ForeignToplevelWeakHandle] to strong [ForeignToplevelHandle]
    pub fn upgrade(&self) -> Option<ForeignToplevelHandle> {
        Some(ForeignToplevelHandle {
            inner: self.inner.upgrade()?,
        })
    }
}

/// Handle of a toplevel, used to update title or app_id, after initial handle creation
///
/// eg.
/// ```no_run
/// use smithay::wayland::foreign_toplevel_list::ForeignToplevelHandle;
/// let handle: ForeignToplevelHandle = todo!();
///
/// // Handle can be used to update title and app_id
/// handle.send_title("abc");
/// handle.send_app_id("com.example");
/// handle.send_done();
///
/// // Handle can also be used to close the window, after this call the handle will become inert,
/// // and the handle will no longer be announced to clients
/// handle.send_closed();
/// ```
#[derive(Debug, Clone)]
pub struct ForeignToplevelHandle {
    inner: Arc<(Mutex<ForeignToplevelHandleInner>, UserDataMap)>,
}

impl ForeignToplevelHandle {
    fn new(
        identifier: String,
        title: String,
        app_id: String,
        instances: Vec<Weak<ExtForeignToplevelHandleV1>>,
    ) -> Self {
        Self {
            inner: Arc::new((
                Mutex::new(ForeignToplevelHandleInner {
                    identifier,
                    title,
                    app_id,
                    instances,
                    closed: false,
                }),
                UserDataMap::new(),
            )),
        }
    }

    /// Downgrade strong [ForeignToplevelHandle] to weak [ForeignToplevelWeakHandle]
    pub fn downgrade(&self) -> ForeignToplevelWeakHandle {
        ForeignToplevelWeakHandle {
            inner: Arc::downgrade(&self.inner),
        }
    }

    /// Attempt to retrieve [ForeignToplevelHandle] from an existing resource
    pub fn from_resource(resource: &ExtForeignToplevelHandleV1) -> Option<Self> {
        resource.data::<Self>().cloned()
    }

    /// Retrieve [`ExtForeignToplevelHandleV1`]
    /// instances for this handle.
    pub fn resources(&self) -> Vec<ExtForeignToplevelHandleV1> {
        let inner = self.inner.0.lock().unwrap();
        inner
            .instances
            .iter()
            .filter_map(|weak| weak.upgrade().ok())
            .collect()
    }

    /// Retrieve [`ExtForeignToplevelHandleV1`]
    /// instances for this handle of a given [`Client`].
    pub fn resources_for_client(&self, client: &Client) -> Vec<ExtForeignToplevelHandleV1> {
        self.resources()
            .into_iter()
            .filter(|handle| handle.client().as_ref().is_some_and(|c| c == client))
            .collect()
    }

    /// Access the [UserDataMap] associated with this [ForeignToplevelHandle]
    pub fn user_data(&self) -> &UserDataMap {
        &self.inner.1
    }

    /// The title of the toplevel has changed.
    ///
    /// [Self::send_done] has to be called to finalize the update
    pub fn send_title(&self, title: &str) {
        let mut inner = self.inner.0.lock().unwrap();
        if inner.title == title {
            return;
        }

        inner.title = title.to_string();

        for toplevel in inner.instances.iter() {
            if let Ok(toplevel) = toplevel.upgrade() {
                toplevel.title(title.to_string());
            }
        }
    }

    /// The app_id of the toplevel has changed.
    ///
    /// [Self::send_done] has to be called to finalize the update
    pub fn send_app_id(&self, app_id: &str) {
        let mut inner = self.inner.0.lock().unwrap();
        if inner.app_id == app_id {
            return;
        }

        inner.app_id = app_id.to_string();

        for toplevel in inner.instances.iter() {
            if let Ok(toplevel) = toplevel.upgrade() {
                toplevel.app_id(app_id.to_string());
            }
        }
    }

    /// This event is should be sent after all changes in the toplevel state have been sent.
    pub fn send_done(&self) {
        let inner = self.inner.0.lock().unwrap();
        for toplevel in inner.instances.iter() {
            if let Ok(toplevel) = toplevel.upgrade() {
                toplevel.done();
            }
        }
    }

    /// The toplevel has been closed
    pub fn send_closed(&self) {
        self.inner.0.lock().unwrap().send_closed();
    }

    /// A stable identifier for a toplevel
    pub fn identifier(&self) -> String {
        self.inner.0.lock().unwrap().identifier.clone()
    }

    /// The title of the toplevel
    pub fn title(&self) -> String {
        self.inner.0.lock().unwrap().title.clone()
    }

    /// The app id of the toplevel
    pub fn app_id(&self) -> String {
        self.inner.0.lock().unwrap().app_id.clone()
    }

    /// The toplevel has been closed
    pub fn is_closed(&self) -> bool {
        self.inner.0.lock().unwrap().closed
    }

    fn init_new_instance(&self, toplevel: ExtForeignToplevelHandleV1) {
        debug_assert!(
            !self.is_closed(),
            "No handles should ever be created for closed toplevel"
        );

        toplevel.identifier(self.identifier());
        toplevel.title(self.title());
        toplevel.app_id(self.app_id());
        toplevel.done();

        self.inner.0.lock().unwrap().instances.push(toplevel.downgrade());
    }

    fn remove_instance(&self, instance: &ExtForeignToplevelHandleV1) {
        let mut inner = self.inner.0.lock().unwrap();
        if let Some(pos) = inner.instances.iter().position(|i| i == instance) {
            inner.instances.remove(pos);
        }
    }
}

/// State of the [ExtForeignToplevelListV1] global
#[derive(Debug)]
pub struct ForeignToplevelListState {
    global: GlobalId,
    toplevels: Vec<ForeignToplevelWeakHandle>,
    list_instances: Vec<ExtForeignToplevelListV1>,
    dh: DisplayHandle,
}

impl ForeignToplevelListState {
    /// Register new [ExtForeignToplevelListV1] global
    pub fn new<D: ForeignToplevelListHandler>(dh: &DisplayHandle) -> Self {
        Self::new_with_filter::<D>(dh, |_| true)
    }

    /// Register new [ExtForeignToplevelListV1] global with filter
    pub fn new_with_filter<D: ForeignToplevelListHandler>(
        dh: &DisplayHandle,
        can_view: impl Fn(&Client) -> bool + Send + Sync + 'static,
    ) -> Self {
        let global = dh.create_global::<D, ExtForeignToplevelListV1, _>(
            1,
            ForeignToplevelListGlobalData {
                filter: Box::new(can_view),
            },
        );

        Self {
            global,
            toplevels: Vec::new(),
            list_instances: Vec::new(),
            dh: dh.clone(),
        }
    }

    /// [ExtForeignToplevelListV1] GlobalId getter
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }

    /// This event is emitted whenever a new toplevel window is created.
    /// It is emitted for all toplevels, regardless of the app that has created them.
    pub fn new_toplevel<D: ForeignToplevelListHandler>(
        &mut self,
        title: impl Into<String>,
        app_id: impl Into<String>,
    ) -> ForeignToplevelHandle {
        let handle = ForeignToplevelHandle::new(
            Alphanumeric.sample_string(&mut rand::rng(), 32),
            title.into(),
            app_id.into(),
            Vec::with_capacity(self.list_instances.len()),
        );

        for instance in &self.list_instances {
            let Ok(client) = self.dh.get_client(instance.id()) else {
                continue;
            };

            let Ok(toplevel) = client.create_resource::<ExtForeignToplevelHandleV1, _, D>(
                &self.dh,
                instance.version(),
                handle.clone(),
            ) else {
                continue;
            };

            instance.toplevel(&toplevel);
            handle.init_new_instance(toplevel);
        }

        self.toplevels.push(handle.downgrade());

        handle
    }

    /// Remove the toplevel, and send closed event if needed
    ///
    /// Alternatively, you can just call [ForeignToplevelHandle::send_closed] and the handle will be
    /// lazely cleaned up, either by [Self::cleanup_closed_handles], or during next global bind
    pub fn remove_toplevel(&mut self, handle: &ForeignToplevelHandle) {
        handle.send_closed();

        if let Some(pos) = self
            .toplevels
            .iter()
            .filter_map(|h| h.upgrade())
            .position(|h| Arc::ptr_eq(&h.inner, &handle.inner))
        {
            self.toplevels.remove(pos);
        }
    }

    /// Auto cleanup closed handles
    ///
    /// This is not needed if you already manually remove each handle with [Self::remove_toplevel]
    pub fn cleanup_closed_handles(&mut self) {
        self.toplevels.retain(|handle| {
            let Some(handle) = handle.upgrade() else {
                return false;
            };
            !handle.is_closed()
        });
    }
}

/// Glabal data of [ExtForeignToplevelListV1]
pub struct ForeignToplevelListGlobalData {
    filter: Box<dyn Fn(&Client) -> bool + Send + Sync>,
}

impl std::fmt::Debug for ForeignToplevelListGlobalData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForeignToplevelListGlobalData")
            .finish_non_exhaustive()
    }
}

impl<D: ForeignToplevelListHandler> GlobalDispatch<ExtForeignToplevelListV1, ForeignToplevelListGlobalData, D>
    for ForeignToplevelListState
{
    fn bind(
        state: &mut D,
        dh: &DisplayHandle,
        client: &Client,
        resource: New<ExtForeignToplevelListV1>,
        _global_data: &ForeignToplevelListGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let instance = data_init.init(resource, ());

        let state = state.foreign_toplevel_list_state();

        state.toplevels.retain(|handle| {
            let Some(handle) = handle.upgrade() else {
                // Cleanup dead handles
                return false;
            };

            if handle.is_closed() {
                // Cleanup closed handles
                return false;
            }

            if let Ok(toplevel) = client.create_resource::<ExtForeignToplevelHandleV1, _, D>(
                dh,
                instance.version(),
                handle.clone(),
            ) {
                instance.toplevel(&toplevel);
                handle.init_new_instance(toplevel);
            }

            true
        });

        state.list_instances.push(instance);
    }

    fn can_view(client: Client, global_data: &ForeignToplevelListGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D: ForeignToplevelListHandler> Dispatch<ExtForeignToplevelListV1, (), D> for ForeignToplevelListState {
    fn request(
        state: &mut D,
        client: &wayland_server::Client,
        manager: &ExtForeignToplevelListV1,
        request: ext_foreign_toplevel_list_v1::Request,
        data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            ext_foreign_toplevel_list_v1::Request::Stop => {
                Self::destroyed(state, client.id(), manager, data);
                manager.finished();
            }
            ext_foreign_toplevel_list_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(state: &mut D, _client: ClientId, resource: &ExtForeignToplevelListV1, _data: &()) {
        state
            .foreign_toplevel_list_state()
            .list_instances
            .retain(|i| i != resource);
    }
}

impl<D: ForeignToplevelListHandler> Dispatch<ExtForeignToplevelHandleV1, ForeignToplevelHandle, D>
    for ForeignToplevelListState
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _context: &ExtForeignToplevelHandleV1,
        request: ext_foreign_toplevel_handle_v1::Request,
        _data: &ForeignToplevelHandle,
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            ext_foreign_toplevel_handle_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _client: ClientId,
        resource: &ExtForeignToplevelHandleV1,
        handle: &ForeignToplevelHandle,
    ) {
        handle.remove_instance(resource);
    }
}

/// Macro to delegate implementation of the xdg toplevel icon to [ForeignToplevelListState].
///
/// You must also implement [ForeignToplevelListHandler] to use this.
#[macro_export]
macro_rules! delegate_foreign_toplevel_list {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::ext::foreign_toplevel_list::v1::server::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1: $crate::wayland::foreign_toplevel_list::ForeignToplevelListGlobalData
        ] => $crate::wayland::foreign_toplevel_list::ForeignToplevelListState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::ext::foreign_toplevel_list::v1::server::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1: ()
        ] => $crate::wayland::foreign_toplevel_list::ForeignToplevelListState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::ext::foreign_toplevel_list::v1::server::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1: $crate::wayland::foreign_toplevel_list::ForeignToplevelHandle
        ] => $crate::wayland::foreign_toplevel_list::ForeignToplevelListState);
    };
}
