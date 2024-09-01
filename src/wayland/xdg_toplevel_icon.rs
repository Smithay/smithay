//! XDG Toplevel Icon
//!
//! This protocol allows clients to set icons for their toplevel surfaces either via the XDG icon stock (using an icon name), or from pixel data.
//!
//! In order to advertise toplevel icon global call [XdgToplevelIconManager::new] and delegate
//! events to it with [delegate_xdg_toplevel_icon].
//! Currently attached icon is available in double-buffered [ToplevelIconCachedState]

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use wayland_protocols::xdg::{
    shell::server::xdg_toplevel::XdgToplevel,
    toplevel_icon::v1::server::{
        xdg_toplevel_icon_manager_v1::{self, XdgToplevelIconManagerV1},
        xdg_toplevel_icon_v1::{self, XdgToplevelIconV1},
    },
};
use wayland_server::{
    backend::{ClientId, GlobalId},
    protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface},
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::{
    utils::HookId,
    wayland::{
        compositor::{self, Cacheable},
        shell::xdg::XdgShellSurfaceUserData,
        shm::ShmBufferUserData,
    },
};

/// Handler trait for xdg toplevel icon events.
pub trait XdgToplevelIconHandler:
    GlobalDispatch<XdgToplevelIconManagerV1, XdgToplevelIconManagerUserData>
    + Dispatch<XdgToplevelIconManagerV1, ()>
    + Dispatch<XdgToplevelIconV1, XdgToplevelIconUserData>
    + 'static
{
    /// Called when icon becomes pending, and awaits surface commit.
    /// Icon is stored in wl_surface [ToplevelIconCachedState]
    fn set_icon(&mut self, toplevel: XdgToplevel, wl_surface: WlSurface) {
        let _ = toplevel;
        let _ = wl_surface;
    }
}

/// Data associated with WlSurface
/// Represents the client pending state
///
/// ```no_run
/// use smithay::wayland::compositor;
/// use smithay::wayland::xdg_toplevel_icon::ToplevelIconCachedState;
///
/// # let wl_surface = todo!();
/// compositor::with_states(&wl_surface, |states| {
///     let mut data = states.cached_state.get::<ToplevelIconCachedState>();
///     dbg!(data.current().icon_name());
/// });
/// ```
#[derive(Debug, Clone, Default)]
pub struct ToplevelIconCachedState {
    icon: Option<XdgToplevelIconV1>,
}

impl ToplevelIconCachedState {
    fn data(&self) -> Option<&IconData> {
        let icon = self.icon.as_ref()?;
        let data = icon
            .data::<XdgToplevelIconUserData>()
            .unwrap()
            .constructed
            .get()
            .unwrap();
        Some(data)
    }

    /// Icon name getter
    pub fn icon_name(&self) -> Option<&str> {
        self.data()?.icon_name.as_deref()
    }

    /// List of icon buffers and buffer scale
    pub fn buffers(&self) -> &[(WlBuffer, i32)] {
        let Some(data) = self.data() else {
            return &[];
        };
        &data.buffers
    }
}

impl Cacheable for ToplevelIconCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        self.clone()
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

#[derive(Debug, Default)]
struct IconData {
    icon_name: Option<String>,
    buffers: Vec<(WlBuffer, i32)>,
}

/// User data of [XdgToplevelIconV1]
#[derive(Debug, Default)]
pub struct XdgToplevelIconUserData {
    builder: Mutex<IconData>,
    constructed: once_cell::sync::OnceCell<IconData>,
    buffer_destruction_hooks: Mutex<Vec<(WlBuffer, HookId)>>,
}

impl XdgToplevelIconUserData {
    fn freeze(&self) {
        if self.constructed.get().is_none() {
            let data = std::mem::take(&mut *self.builder.lock().unwrap());
            self.constructed.set(data).unwrap();
        }
    }

    fn is_immutable(&self) -> bool {
        self.constructed.get().is_some()
    }

    fn set_icon_name(&self, icon_name: String) {
        debug_assert!(!self.is_immutable());
        self.builder.lock().unwrap().icon_name = Some(icon_name)
    }

    fn register_buffer_destruction_hook(
        &self,
        buffer: WlBuffer,
        shm: &ShmBufferUserData,
        hook: impl Fn() + Send + Sync + 'static,
    ) {
        let buffer_destruction_hook = shm.add_destruction_hook(move |_, _| hook());
        self.buffer_destruction_hooks
            .lock()
            .unwrap()
            .push((buffer, buffer_destruction_hook));
    }

    fn unregister_buffer_destruction_hook(&self, buffer: &WlBuffer) {
        let mut guard = self.buffer_destruction_hooks.lock().unwrap();

        if let Some((buffer, hook)) = guard
            .iter()
            .position(|(b, _)| b == buffer)
            .map(|id| guard.remove(id))
        {
            if let Some(shm) = buffer.data::<ShmBufferUserData>() {
                shm.remove_destruction_hook(hook);
            }
        }
    }

    fn unregister_all_hooks(&self) {
        let mut guard = self.buffer_destruction_hooks.lock().unwrap();

        for (buffer, hook) in guard.drain(..) {
            if let Some(shm) = buffer.data::<ShmBufferUserData>() {
                shm.remove_destruction_hook(hook);
            }
        }
    }

    fn add_buffer(&self, buffer: WlBuffer, scale: i32, shm: &ShmBufferUserData) {
        debug_assert!(!self.is_immutable());

        let mut guard = self.builder.lock().unwrap();
        let list = &mut guard.buffers;

        for (existing_buffer, existing_scale) in list.iter_mut() {
            let existing_data = &existing_buffer.data::<ShmBufferUserData>().unwrap().data;

            let same_width = existing_data.width == shm.data.width;
            let same_height = existing_data.height == shm.data.height;
            let same_scale = *existing_scale == scale;

            // If this icon instance already has a buffer of the same size and scale from a previous 'add_buffer' request,
            // data from the last request overrides the preexisting pixel data.
            if same_width && same_height && same_scale {
                // The existing buffer will not be used, so let's assume we don't care about it's
                // destruction
                self.unregister_buffer_destruction_hook(existing_buffer);
                *existing_buffer = buffer;
                return;
            }
        }

        list.push((buffer, scale));
    }
}

#[derive(Debug, Default)]
struct ManagerGlobalData {
    sizes: HashSet<i32>,
}

/// User data of [XdgToplevelIconManagerV1] global
#[derive(Debug)]
pub struct XdgToplevelIconManagerUserData(Arc<Mutex<ManagerGlobalData>>);

/// Delegate type for handling xdg toplevel icon events.
#[derive(Debug)]
pub struct XdgToplevelIconManager {
    global: GlobalId,
    data: Arc<Mutex<ManagerGlobalData>>,
}

impl XdgToplevelIconManager {
    /// Creates a new delegate type for handling xdg toplevel icon events.
    pub fn new<D: XdgToplevelIconHandler>(display: &DisplayHandle) -> Self {
        let data = Arc::new(Mutex::new(ManagerGlobalData::default()));
        let global = display
            .create_global::<D, XdgToplevelIconManagerV1, _>(1, XdgToplevelIconManagerUserData(data.clone()));
        XdgToplevelIconManager { global, data }
    }

    /// Request given icon size
    ///
    /// - `size` the edge size of the square icon in surface-local coordinates
    pub fn add_icon_size(&mut self, size: i32) {
        self.data.lock().unwrap().sizes.insert(size);
    }

    /// Returns the [XdgToplevelIconManagerV1] global id.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D: XdgToplevelIconHandler> GlobalDispatch<XdgToplevelIconManagerV1, XdgToplevelIconManagerUserData, D>
    for XdgToplevelIconManager
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<XdgToplevelIconManagerV1>,
        data: &XdgToplevelIconManagerUserData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let manager = data_init.init(resource, ());

        for size in data.0.lock().unwrap().sizes.iter() {
            manager.icon_size(*size);
        }
        manager.done();
    }
}

impl<D: XdgToplevelIconHandler> Dispatch<XdgToplevelIconManagerV1, (), D> for XdgToplevelIconManager {
    fn request(
        state: &mut D,
        _: &Client,
        _resource: &XdgToplevelIconManagerV1,
        request: xdg_toplevel_icon_manager_v1::Request,
        _: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use xdg_toplevel_icon_manager_v1::Request;

        match request {
            Request::CreateIcon { id } => {
                data_init.init(id, XdgToplevelIconUserData::default());
            }
            Request::SetIcon { toplevel, icon } => {
                let data = toplevel.data::<XdgShellSurfaceUserData>().unwrap();
                let wl_surface = data.wl_surface.clone();

                compositor::with_states(&data.wl_surface, |states| {
                    let mut state = states.cached_state.get::<ToplevelIconCachedState>();
                    let pending = state.pending();
                    match icon {
                        Some(icon) => {
                            let icon_data = icon.data::<XdgToplevelIconUserData>().unwrap();
                            icon_data.freeze();
                            pending.icon = Some(icon);
                        }
                        None => {
                            *pending = Default::default();
                        }
                    }
                });

                state.set_icon(toplevel, wl_surface);
            }
            Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D: XdgToplevelIconHandler> Dispatch<XdgToplevelIconV1, XdgToplevelIconUserData, D>
    for XdgToplevelIconManager
{
    fn request(
        _state: &mut D,
        _: &Client,
        icon: &XdgToplevelIconV1,
        request: xdg_toplevel_icon_v1::Request,
        data: &XdgToplevelIconUserData,
        dh: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        use xdg_toplevel_icon_v1::Request;

        match request {
            Request::SetName { icon_name } => {
                if data.is_immutable() {
                    dh.post_error(
                        icon,
                        xdg_toplevel_icon_v1::Error::Immutable as u32,
                        "Request made after the icon has been assigned to a toplevel via 'set_icon'"
                            .to_string(),
                    );
                }

                data.set_icon_name(icon_name);
            }
            Request::AddBuffer { buffer, scale } => {
                if data.is_immutable() {
                    dh.post_error(
                        icon,
                        xdg_toplevel_icon_v1::Error::Immutable as u32,
                        "Request made after the icon has been assigned to a toplevel via 'set_icon'"
                            .to_string(),
                    );
                }

                let Some(shm) = buffer.data::<ShmBufferUserData>() else {
                    dh.post_error(
                        icon,
                        xdg_toplevel_icon_v1::Error::InvalidBuffer as u32,
                        "The wl_buffer must be backed by wl_shm".to_string(),
                    );
                    return;
                };

                if shm.data.width != shm.data.height {
                    dh.post_error(
                        icon,
                        xdg_toplevel_icon_v1::Error::InvalidBuffer as u32,
                        "The wl_buffer must be a square".to_string(),
                    );
                    return;
                };

                // Let's listen for buffer destruction event to catch no_buffer protocol error
                // This hook has to be unregistered once the icon is destroyed
                data.register_buffer_destruction_hook(buffer.clone(), shm, {
                    let icon = icon.clone();
                    move || {
                        icon.post_error(
                            xdg_toplevel_icon_v1::Error::NoBuffer,
                            "The provided buffer has been destroyed before the toplevel icon",
                        )
                    }
                });
                data.add_buffer(buffer.clone(), scale, shm);
            }
            Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _client: ClientId,
        _resource: &XdgToplevelIconV1,
        data: &XdgToplevelIconUserData,
    ) {
        data.unregister_all_hooks();
    }
}

/// Macro to delegate implementation of the xdg toplevel icon to [`XdgToplevelIconState`].
///
/// You must also implement [`XdgToplevelIconHandler`] to use this.
#[macro_export]
macro_rules! delegate_xdg_toplevel_icon {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::toplevel_icon::v1::server::xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1: XdgToplevelIconManagerUserData
        ] => $crate::wayland::xdg_toplevel_icon::XdgToplevelIconState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::toplevel_icon::v1::server::xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1: ()
        ] => $crate::wayland::xdg_toplevel_icon::XdgToplevelIconState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::toplevel_icon::v1::server::xdg_toplevel_icon_v1::XdgToplevelIconV1: XdgToplevelIconUserData
        ] => $crate::wayland::xdg_toplevel_icon::XdgToplevelIconState);
    };
}
