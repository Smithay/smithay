//! Helpers for handling the xwayland shell protocol
//!
//! # Example
//!
//! ```
//! use smithay::wayland::xwayland_shell::{
//!     XWaylandShellHandler,
//!     XWaylandShellState,
//! };
//! use smithay::delegate_xwayland_shell;
//!
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the global:
//! XWaylandShellState::new::<State>(
//!     &display.handle(),
//! );
//! #
//! # use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
//!
//! impl XWaylandShellHandler for State {
//!     fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
//!         // Return a reference to the state we created earlier.
//!         todo!()
//!     }
//!
//!     fn surface_associated(&mut self, _surface: WlSurface, _serial: u64) {
//!         // Called when XWayland has associated an X11 window with a wl_surface.
//!         todo!()
//!     }
//! }
//!
//! // implement Dispatch for your state.
//! delegate_xwayland_shell!(State);
//! ```

use std::collections::HashMap;

use wayland_protocols::xwayland::shell::v1::server::{
    xwayland_shell_v1::{self, XwaylandShellV1},
    xwayland_surface_v1::{self, XwaylandSurfaceV1},
};
use wayland_server::{
    backend::GlobalId,
    protocol::wl_surface::{self, WlSurface},
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::{wayland::compositor, xwayland::XWaylandClientData};

/// The role for an xwayland-associated surface.
pub const XWAYLAND_SHELL_ROLE: &str = "xwayland_shell";

const VERSION: u32 = 1;

/// The global for the xwayland shell protocol.
#[derive(Debug, Clone)]
pub struct XWaylandShellState {
    global: GlobalId,
    by_serial: HashMap<u64, WlSurface>,
}

impl XWaylandShellState {
    /// Registers a new [XwaylandShellV1] global. Only XWayland clients will be
    /// able to bind it.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<XwaylandShellV1, ()>,
        D: Dispatch<XwaylandShellV1, ()>,
        D: Dispatch<XwaylandSurfaceV1, XWaylandSurfaceUserData>,
        D: 'static,
    {
        let global = display.create_global::<D, XwaylandShellV1, _>(VERSION, ());
        Self {
            global,
            by_serial: HashMap::new(),
        }
    }

    /// Retrieve a handle for the [XwaylandShellV1] global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }

    /// Retrieves the surface for a given serial.
    pub fn surface_for_serial(&self, serial: u64) -> Option<WlSurface> {
        self.by_serial.get(&serial).cloned()
    }
}

/// Userdata for an xwayland shell surface.
#[derive(Debug, Clone)]
pub struct XWaylandSurfaceUserData {
    pub(crate) wl_surface: wl_surface::WlSurface,
}

/// Handler for the xwayland shell protocol.
pub trait XWaylandShellHandler {
    /// Retrieves the global state.
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState;

    /// An X11 window has been associated with a wayland surface. This doesn't
    /// take effect until the wl_surface is committed.
    fn surface_associated(&mut self, _surface: wl_surface::WlSurface, _serial: u64) {}
}

/// Represents a pending X11 serial, used to associate X11 windows with wayland
/// surfaces.
#[derive(Debug, Default, Clone, Copy)]
pub struct XWaylandShellCachedState {
    /// The serial of the matching X11 window.
    pub serial: Option<u64>,
}

impl compositor::Cacheable for XWaylandShellCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        *self
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

impl<D> GlobalDispatch<XwaylandShellV1, (), D> for XWaylandShellState
where
    D: GlobalDispatch<XwaylandShellV1, ()>,
    D: Dispatch<XwaylandShellV1, ()>,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<XwaylandShellV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _global_data: &()) -> bool {
        client.get_data::<XWaylandClientData>().is_some()
    }
}

impl<D> Dispatch<XwaylandShellV1, (), D> for XWaylandShellState
where
    D: Dispatch<XwaylandShellV1, ()>,
    D: Dispatch<XwaylandSurfaceV1, XWaylandSurfaceUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        resource: &XwaylandShellV1,
        request: <XwaylandShellV1 as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xwayland_shell_v1::Request::GetXwaylandSurface { id, surface } => {
                if compositor::give_role(&surface, XWAYLAND_SHELL_ROLE).is_err() {
                    resource.post_error(xwayland_shell_v1::Error::Role, "Surface already has a role.");
                    return;
                }

                data_init.init(id, XWaylandSurfaceUserData { wl_surface: surface });
                // We call the handler callback once the serial is set.
            }
            xwayland_shell_v1::Request::Destroy => {
                // The child objects created via this interface are unaffected.
            }
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<XwaylandSurfaceV1, XWaylandSurfaceUserData, D> for XWaylandShellState
where
    D: Dispatch<XwaylandSurfaceV1, XWaylandSurfaceUserData>,
    D: XWaylandShellHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &XwaylandSurfaceV1,
        request: <XwaylandSurfaceV1 as Resource>::Request,
        data: &XWaylandSurfaceUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xwayland_surface_v1::Request::SetSerial { serial_lo, serial_hi } => {
                let serial = u64::from(serial_lo) | (u64::from(serial_hi) << 32);

                compositor::with_states(&data.wl_surface, |states| {
                    states.cached_state.pending::<XWaylandShellCachedState>().serial = Some(serial);
                });

                compositor::add_pre_commit_hook::<D, _>(&data.wl_surface, register_serial_commit_hook);

                XWaylandShellHandler::surface_associated(state, data.wl_surface.clone(), serial);
            }
            xwayland_surface_v1::Request::Destroy => {
                // Any already existing associations are unaffected.
            }
            _ => unreachable!(),
        }
    }
}

fn register_serial_commit_hook<D: XWaylandShellHandler + 'static>(
    state: &mut D,
    _dh: &DisplayHandle,
    surface: &WlSurface,
) {
    if let Some(serial) = compositor::with_states(surface, |states| {
        states.cached_state.pending::<XWaylandShellCachedState>().serial
    }) {
        XWaylandShellHandler::xwayland_shell_state(state)
            .by_serial
            .insert(serial, surface.clone());
    }
}

/// Macro to delegate implementation of the xwayland keyboard grab protocol
#[macro_export]
macro_rules! delegate_xwayland_shell {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xwayland::shell::v1::server::xwayland_shell_v1::XwaylandShellV1: ()
        ] => $crate::wayland::xwayland_shell::XWaylandShellState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xwayland::shell::v1::server::xwayland_shell_v1::XwaylandShellV1: ()
        ] => $crate::wayland::xwayland_shell::XWaylandShellState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xwayland::shell::v1::server::xwayland_surface_v1::XwaylandSurfaceV1: $crate::wayland::xwayland_shell::XWaylandSurfaceUserData
        ] => $crate::wayland::xwayland_shell::XWaylandShellState);
    };
}
