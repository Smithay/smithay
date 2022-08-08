//! XDG Window decoration manager
//!
//! This interface allows a compositor to announce support for server-side decorations.
//!
//! A client can use this protocol to request being decorated by a supporting compositor.
//!
//!
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use smithay::{delegate_xdg_decoration, delegate_xdg_shell};
//! use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellHandler};
//! # use smithay::utils::Serial;
//! # use smithay::wayland::shell::xdg::{XdgShellState, PopupSurface, PositionerState};
//! # use smithay::reexports::wayland_server::protocol::wl_seat;
//! use smithay::wayland::shell::xdg::decoration::{XdgDecorationState, XdgDecorationHandler};
//! use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
//!
//! # struct State { decoration_state: XdgDecorationState }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! // Create a decoration state
//! let decoration_state = XdgDecorationState::new::<State, _>(
//!     &display.handle(),
//!     None,
//! );
//!
//! // store that state inside your compositor state
//! // ...
//!
//! // implement the necessary traits
//! impl XdgShellHandler for State {
//!     # fn xdg_shell_state(&mut self) -> &mut XdgShellState { unimplemented!() }
//!     # fn new_toplevel(&mut self, dh: &wayland_server::DisplayHandle, surface: ToplevelSurface) { unimplemented!() }
//!     # fn new_popup(
//!     #     &mut self,
//!     #     dh: &wayland_server::DisplayHandle,
//!     #     surface: PopupSurface,
//!     #     positioner: PositionerState,
//!     # ) { unimplemented!() }
//!     # fn grab(
//!     #     &mut self,
//!     #     dh: &wayland_server::DisplayHandle,
//!     #     surface: PopupSurface,
//!     #     seat: wl_seat::WlSeat,
//!     #     serial: Serial,
//!     # ) { unimplemented!() }
//!     // ...
//! }
//! impl XdgDecorationHandler for State {
//!     fn new_decoration(&mut self, dh: &wayland_server::DisplayHandle, toplevel: ToplevelSurface) {
//!         toplevel.with_pending_state(|state| {
//!             // Advertise server side decoration
//!             state.decoration_mode = Some(Mode::ServerSide);
//!         });
//!         toplevel.send_configure();
//!     }
//!     fn request_mode(&mut self, dh: &wayland_server::DisplayHandle, toplevel: ToplevelSurface, mode: Mode) { /* ... */ }
//!     fn unset_mode(&mut self, dh: &wayland_server::DisplayHandle, toplevel: ToplevelSurface) { /* ... */ }
//! }
//! delegate_xdg_shell!(State);
//! delegate_xdg_decoration!(State);
//!
//! // You are ready to go!  
// TODO: Describe how to change decoration mode.

use wayland_protocols::xdg::decoration::zv1::server::{
    zxdg_decoration_manager_v1,
    zxdg_toplevel_decoration_v1::{self, Mode},
};
use wayland_server::{
    backend::GlobalId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

use super::{ToplevelSurface, XdgShellHandler};
use crate::wayland::shell::xdg::XdgShellSurfaceUserData;

/// Delegate type for handling xdg decoration events.
#[derive(Debug)]
pub struct XdgDecorationState {
    _logger: ::slog::Logger,
    global: GlobalId,
}

impl XdgDecorationState {
    /// Creates a new delegate type for handling xdg decoration events.
    ///
    /// A global id is also returned to allow destroying the global in the future.
    pub fn new<D, L>(display: &DisplayHandle, logger: L) -> XdgDecorationState
    where
        D: GlobalDispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, ()>
            + Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, ()>
            + 'static,
        L: Into<Option<::slog::Logger>>,
    {
        let _logger = crate::slog_or_fallback(logger);
        let global =
            display.create_global::<D, zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, _>(1, ());

        XdgDecorationState { _logger, global }
    }

    /// Returns the xdg-decoration global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// Handler trait for xdg decoration events.
pub trait XdgDecorationHandler {
    /// Notification the client supports server side decoration on the toplevel.
    fn new_decoration(&mut self, dh: &DisplayHandle, toplevel: ToplevelSurface);

    /// Notification the client prefers the provided decoration decoration mode on the toplevel.
    fn request_mode(&mut self, dh: &DisplayHandle, toplevel: ToplevelSurface, mode: Mode);

    /// Notification the client does not prefer a particular decoration mode on the toplevel.
    fn unset_mode(&mut self, dh: &DisplayHandle, toplevel: ToplevelSurface);
}

/// Macro to delegate implementation of the xdg decoration to [`XdgDecorationState`].
///
/// You must also implement [`XdgDecorationHandler`] to use this.
#[macro_export]
macro_rules! delegate_xdg_decoration {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1: ()
        ] => $crate::wayland::shell::xdg::decoration::XdgDecorationState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1: ()
        ] => $crate::wayland::shell::xdg::decoration::XdgDecorationState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1: $crate::wayland::shell::xdg::ToplevelSurface
        ] => $crate::wayland::shell::xdg::decoration::XdgDecorationState);
    };
}

pub(super) fn send_decoration_configure(
    id: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    mode: Mode,
) {
    id.configure(mode)
}

impl<D> GlobalDispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, (), D> for XdgDecorationState
where
    D: GlobalDispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, ()>
        + Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, ()>
        + Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ToplevelSurface>
        + XdgShellHandler
        + XdgDecorationHandler
        + 'static,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, (), D> for XdgDecorationState
where
    D: Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, ()>
        + Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ToplevelSurface>
        + XdgShellHandler
        + XdgDecorationHandler
        + 'static,
{
    fn request(
        state: &mut D,
        _: &Client,
        resource: &zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
        request: zxdg_decoration_manager_v1::Request,
        _: &(),
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use self::zxdg_decoration_manager_v1::Request;

        match request {
            Request::GetToplevelDecoration { id, toplevel } => {
                let data = toplevel.data::<XdgShellSurfaceUserData>().unwrap();

                let mut decoration_guard = data.decoration.lock().unwrap();

                if decoration_guard.is_some() {
                    resource.post_error(
                        zxdg_toplevel_decoration_v1::Error::AlreadyConstructed,
                        "toplevel decoration is already constructed",
                    );
                    return;
                }

                let toplevel = state.xdg_shell_state().get_toplevel(&toplevel).unwrap();
                let toplevel_decoration = data_init.init(id, toplevel.clone());

                *decoration_guard = Some(toplevel_decoration);
                drop(decoration_guard);

                state.new_decoration(dh, toplevel);
            }

            Request::Destroy => {}

            _ => unreachable!(),
        }
    }
}

// zxdg_toplevel_decoration_v1

impl<D> Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ToplevelSurface, D>
    for XdgDecorationState
where
    D: Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ToplevelSurface>
        + XdgDecorationHandler,
{
    fn request(
        state: &mut D,
        _: &Client,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        request: zxdg_toplevel_decoration_v1::Request,
        data: &ToplevelSurface,
        dh: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        use self::zxdg_toplevel_decoration_v1::Request;

        match request {
            Request::SetMode { mode } => {
                if let WEnum::Value(mode) = mode {
                    state.request_mode(dh, data.clone(), mode);
                }
            }

            Request::UnsetMode => {
                state.unset_mode(dh, data.clone());
            }

            Request::Destroy => {
                if let Some(data) = data.xdg_toplevel().data::<XdgShellSurfaceUserData>() {
                    data.decoration.lock().unwrap().take();
                }
            }

            _ => unreachable!(),
        }
    }
}
