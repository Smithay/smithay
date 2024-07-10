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
//! use smithay::xwayland::xwm::{XwmId, X11Surface};
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
//!     fn surface_associated(&mut self, _xwm_id: XwmId, _surface: WlSurface, _window: X11Surface) {
//!         // Called when XWayland has associated an X11 window with a wl_surface.
//!         todo!()
//!     }
//! }
//!
//! #  use smithay::wayland::selection::SelectionTarget;
//! #  use smithay::xwayland::{XWayland, XWaylandEvent, X11Wm, XwmHandler, xwm::{ResizeEdge, Reorder}};
//! #  use smithay::utils::{Rectangle, Logical};
//! #  use std::os::unix::io::OwnedFd;
//! #  use std::process::Stdio;
//!
//! impl XwmHandler for State {
//!     fn xwm_state(&mut self, xwm: XwmId) -> &mut X11Wm {
//!         // ...
//! #       unreachable!()
//!     }
//!     fn new_window(&mut self, xwm: XwmId, window: X11Surface) { /* ... */ }
//!     fn new_override_redirect_window(&mut self, xwm: XwmId, window: X11Surface) { /* ... */ }
//!     fn map_window_request(&mut self, xwm: XwmId, window: X11Surface) { /* ... */ }
//!     fn mapped_override_redirect_window(&mut self, xwm: XwmId, window: X11Surface) { /* ... */ }
//!     fn unmapped_window(&mut self, xwm: XwmId, window: X11Surface) { /* ... */ }
//!     fn destroyed_window(&mut self, xwm: XwmId, window: X11Surface) { /* ... */ }
//!     fn configure_request(&mut self, xwm: XwmId, window: X11Surface, x: Option<i32>, y: Option<i32>, w: Option<u32>, h: Option<u32>, reorder: Option<Reorder>) { /* ... */ }
//!     fn configure_notify(&mut self, xwm: XwmId, window: X11Surface, geometry: Rectangle<i32, Logical>, above: Option<u32>) { /* ... */ }
//!     fn resize_request(&mut self, xwm: XwmId, window: X11Surface, button: u32, resize_edge: ResizeEdge) { /* ... */ }
//!     fn move_request(&mut self, xwm: XwmId, window: X11Surface, button: u32) { /* ... */ }
//!     fn send_selection(&mut self, xwm: XwmId, selection: SelectionTarget, mime_type: String, fd: OwnedFd) { /* ... */ }
//! }
//!
//! // implement Dispatch for your state.
//! delegate_xwayland_shell!(State);
//! ```

use std::collections::HashMap;

use tracing::{debug, warn};
use wayland_protocols::xwayland::shell::v1::server::{
    xwayland_shell_v1::{self, XwaylandShellV1},
    xwayland_surface_v1::{self, XwaylandSurfaceV1},
};
use wayland_server::{
    backend::GlobalId,
    protocol::wl_surface::{self, WlSurface},
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::{
    wayland::compositor,
    xwayland::{xwm::XwmId, X11Surface, XWaylandClientData, XwmHandler},
};

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
    fn surface_associated(&mut self, xwm: XwmId, wl_surface: wl_surface::WlSurface, surface: X11Surface) {
        let _ = (xwm, wl_surface, surface);
    }
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
    D: XWaylandShellHandler + XwmHandler,
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

                compositor::add_pre_commit_hook::<D, _>(&surface, serial_commit_hook);

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
    D: XwmHandler,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &XwaylandSurfaceV1,
        request: <XwaylandSurfaceV1 as Resource>::Request,
        data: &XWaylandSurfaceUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            // In order to match a WlSurface to an X11 window, we need to match a Serial sent both to the X11 window and the WlSurface.
            // This can happen in any order so we need to store the Serial until we have both.
            xwayland_surface_v1::Request::SetSerial { serial_lo, serial_hi } => {
                let serial = u64::from(serial_lo) | (u64::from(serial_hi) << 32);

                // Set the serial on the pending state of surface
                compositor::with_states(&data.wl_surface, |states| {
                    states
                        .cached_state
                        .get::<XWaylandShellCachedState>()
                        .pending()
                        .serial = Some(serial);
                });
            }
            xwayland_surface_v1::Request::Destroy => {
                // Any already existing associations are unaffected.
            }
            _ => unreachable!(),
        }
    }
}

fn serial_commit_hook<D: XWaylandShellHandler + XwmHandler + 'static>(
    state: &mut D,
    _dh: &DisplayHandle,
    surface: &WlSurface,
) {
    if let Some(serial) = compositor::with_states(surface, |states| {
        states
            .cached_state
            .get::<XWaylandShellCachedState>()
            .pending()
            .serial
    }) {
        if let Some(client) = surface.client() {
            // We only care about surfaces created by XWayland.
            if let Some(xwm_id) = client
                .get_data::<XWaylandClientData>()
                .and_then(|data| data.user_data().get::<XwmId>())
            {
                let xwm = XwmHandler::xwm_state(state, *xwm_id);

                // This handles the case that the serial was set on the X11
                // window before surface. To handle the other case, we look for
                // a matching surface when the WL_SURFACE_SERIAL atom is sent.
                if let Some(window) = xwm.unpaired_surfaces.remove(&serial) {
                    if let Some(xsurface) = xwm
                        .windows
                        .iter()
                        .find(|x| x.window_id() == window || x.mapped_window_id() == Some(window))
                        .cloned()
                    {
                        debug!(
                            window = xsurface.window_id(),
                            wl_surface = ?surface.id().protocol_id(),
                            "associated X11 window to wl_surface in commit hook",
                        );

                        xsurface.state.lock().unwrap().wl_surface = Some(surface.clone());

                        XWaylandShellHandler::surface_associated(state, *xwm_id, surface.clone(), xsurface);
                    } else {
                        warn!(
                            window,
                            wl_surface = ?surface.id().protocol_id(),
                            "Unknown X11 window associated to wl_surface in commit hook"
                        )
                    }
                } else {
                    // this is necessary for the atom-handler to look up the matching surface
                    XWaylandShellHandler::xwayland_shell_state(state)
                        .by_serial
                        .insert(serial, surface.clone());
                }
            }
        }
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
