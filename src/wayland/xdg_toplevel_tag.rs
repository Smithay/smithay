//! XDG Toplevel Tag
//!
//! This protocol allows clients to set a tag and description of a toplevel.
//!
//! In order to advertise toplevel tag global call [XdgToplevelTagManager::new] and delegate
//! events to it with [`delegate_xdg_toplevel_tag`][crate::delegate_xdg_toplevel_tag].
//! Currently attached tag is available either via [XdgToplevelTagHandler] or in [XdgToplevelTagSurfaceData]
//!
//! ```
//! use smithay::wayland::xdg_toplevel_tag::{XdgToplevelTagManager, XdgToplevelTagHandler};
//! use wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel;
//! use wayland_server::protocol::wl_surface::WlSurface;
//! use smithay::delegate_xdg_toplevel_tag;
//!
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! XdgToplevelTagManager::new::<State>(
//!     &display.handle(),
//! );
//!
//! impl XdgToplevelTagHandler for State {
//!     fn set_tag(&mut self, toplevel: XdgToplevel, tag: String) {
//!         dbg!(tag);
//!     }
//!
//!     fn set_description(&mut self, toplevel: XdgToplevel, description: String) {
//!         dbg!(description);
//!     }
//! }
//!
//! delegate_xdg_toplevel_tag!(State);
//! ```

use std::sync::{Arc, Mutex};

use wayland_protocols::xdg::{
    shell::server::xdg_toplevel::XdgToplevel,
    toplevel_tag::v1::server::xdg_toplevel_tag_manager_v1::{self, XdgToplevelTagManagerV1},
};

use wayland_server::{
    backend::GlobalId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::wayland::{compositor, shell::xdg::XdgShellSurfaceUserData};

/// Data associated with WlSurface
/// Represents the client pending state
///
/// ```no_run
/// use smithay::wayland::compositor;
/// use smithay::wayland::xdg_toplevel_tag::XdgToplevelTagSurfaceData;
///
/// # let wl_surface = todo!();
/// compositor::with_states(&wl_surface, |states| {
///     let mut data = states.data_map.get::<XdgToplevelTagSurfaceData>().unwrap();
///     dbg!(data.tag());
///     dbg!(data.description());
/// });
/// ```
#[derive(Default, Debug)]
pub struct XdgToplevelTagSurfaceData {
    tag: Mutex<Option<Arc<str>>>,
    description: Mutex<Option<Arc<str>>>,
}

impl XdgToplevelTagSurfaceData {
    /// Get tag of a toplevel.
    ///
    /// The tag may be shown to the user in UI,
    /// so it's preferable for it to be human readable,
    /// but it must be suitable for configuration files and should not be translated.
    ///
    /// Example of a tag would be "main window", "settings", "e-mail composer".
    pub fn tag(&self) -> Option<Arc<str>> {
        self.tag.lock().unwrap().clone()
    }

    /// Get description of a toplevel.
    ///
    /// This description may be shown to the user in UI or read by a screen reader for accessibility purposes.
    pub fn description(&self) -> Option<Arc<str>> {
        self.description.lock().unwrap().clone()
    }
}

/// Handler trait for xdg toplevel tag events.
pub trait XdgToplevelTagHandler:
    GlobalDispatch<XdgToplevelTagManagerV1, ()> + Dispatch<XdgToplevelTagManagerV1, ()> + 'static
{
    /// Toplevel tag was set/updated.
    #[allow(unused)]
    fn set_tag(&mut self, toplevel: XdgToplevel, tag: String) {}

    /// Toplevel description was set/updated.
    #[allow(unused)]
    fn set_description(&mut self, toplevel: XdgToplevel, description: String) {}
}

/// Delegate type for handling xdg toplevel tag events.
#[derive(Debug)]
pub struct XdgToplevelTagManager {
    global: GlobalId,
}

impl XdgToplevelTagManager {
    /// Creates a new delegate type for handling xdg toplevel tag events.
    pub fn new<D: XdgToplevelTagHandler>(display: &DisplayHandle) -> Self {
        let global = display.create_global::<D, XdgToplevelTagManagerV1, _>(1, ());
        XdgToplevelTagManager { global }
    }

    /// Returns the [XdgToplevelTagManagerV1] global id.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D: XdgToplevelTagHandler> GlobalDispatch<XdgToplevelTagManagerV1, (), D> for XdgToplevelTagManager {
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<XdgToplevelTagManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D: XdgToplevelTagHandler> Dispatch<XdgToplevelTagManagerV1, (), D> for XdgToplevelTagManager {
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &XdgToplevelTagManagerV1,
        request: xdg_toplevel_tag_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use xdg_toplevel_tag_manager_v1::Request;

        match request {
            Request::SetToplevelTag { toplevel, tag } => {
                let data = toplevel.data::<XdgShellSurfaceUserData>().unwrap();

                compositor::with_states(&data.wl_surface, |states| {
                    let data = states
                        .data_map
                        .get_or_insert_threadsafe(XdgToplevelTagSurfaceData::default);
                    *data.tag.lock().unwrap() = Some(tag.clone().into());
                });

                state.set_tag(toplevel, tag);
            }
            Request::SetToplevelDescription {
                toplevel,
                description,
            } => {
                let data = toplevel.data::<XdgShellSurfaceUserData>().unwrap();

                compositor::with_states(&data.wl_surface, |states| {
                    let data = states
                        .data_map
                        .get_or_insert_threadsafe(XdgToplevelTagSurfaceData::default);
                    *data.tag.lock().unwrap() = Some(description.clone().into());
                });

                state.set_description(toplevel, description);
            }
            Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

/// Macro to delegate implementation of the xdg toplevel tag to [`XdgToplevelTagManager`].
///
/// You must also implement [`XdgToplevelTagHandler`] to use this.
#[macro_export]
macro_rules! delegate_xdg_toplevel_tag {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::toplevel_tag::v1::server::xdg_toplevel_tag_manager_v1::XdgToplevelTagManagerV1: ()
        ] => $crate::wayland::xdg_toplevel_tag::XdgToplevelTagManager);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::toplevel_tag::v1::server::xdg_toplevel_tag_manager_v1::XdgToplevelTagManagerV1: ()
        ] => $crate::wayland::xdg_toplevel_tag::XdgToplevelTagManager);
    };
}
