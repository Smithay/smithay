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
//! use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellHandler};
//! # use smithay::utils::Serial;
//! # use smithay::wayland::shell::xdg::{XdgShellState, PopupSurface, PositionerState};
//! # use smithay::reexports::wayland_server::protocol::{wl_seat, wl_surface};
//! use smithay::wayland::shell::xdg::decoration::{XdgDecorationState, XdgDecorationHandler};
//! use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
//!
//! # struct State { decoration_state: XdgDecorationState, seat_state: SeatState<Self> }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! // Create a decoration state
//! let decoration_state = XdgDecorationState::new::<State>(
//!     &display.handle(),
//! );
//!
//! // store that state inside your compositor state
//! // ...
//!
//! // implement the necessary traits
//! impl XdgShellHandler for State {
//!     # fn xdg_shell_state(&mut self) -> &mut XdgShellState { unimplemented!() }
//!     # fn new_toplevel(&mut self, surface: ToplevelSurface) { unimplemented!() }
//!     # fn new_popup(
//!     #     &mut self,
//!     #     surface: PopupSurface,
//!     #     positioner: PositionerState,
//!     # ) { unimplemented!() }
//!     # fn grab(
//!     #     &mut self,
//!     #     surface: PopupSurface,
//!     #     seat: wl_seat::WlSeat,
//!     #     serial: Serial,
//!     # ) { unimplemented!() }
//!     # fn reposition_request(
//!     #     &mut self,
//!     #     surface: PopupSurface,
//!     #     positioner: PositionerState,
//!     #     token: u32,
//!     # ) { unimplemented!() }
//!     // ...
//! }
//! impl XdgDecorationHandler for State {
//!     fn new_decoration(&mut self, toplevel: ToplevelSurface) {
//!         toplevel.with_pending_state(|state| {
//!             // Advertise server side decoration
//!             state.decoration_mode = Some(Mode::ServerSide);
//!         });
//!         toplevel.send_configure();
//!     }
//!     fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) { /* ... */ }
//!     fn unset_mode(&mut self, toplevel: ToplevelSurface) { /* ... */ }
//! }
//!
//! use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
//!
//! type Target = wl_surface::WlSurface;
//! impl SeatHandler for State {
//!     type KeyboardFocus = Target;
//!     type PointerFocus = Target;
//!     type TouchFocus = Target;
//!
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//!
//!     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&Target>) {
//!         // handle focus changes, if you need to ...
//!     }
//!     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) {
//!         // handle new images for the cursor ...
//!     }
//! }
//!
//! // You are ready to go!  
// TODO: Describe how to change decoration mode.

use wayland_protocols::xdg::decoration::zv1::server::{
    zxdg_decoration_manager_v1::{self, ZxdgDecorationManagerV1},
    zxdg_toplevel_decoration_v1::{self, Mode, ZxdgToplevelDecorationV1},
};
use wayland_server::{
    backend::GlobalId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

use super::{ToplevelSurface, XdgShellHandler};
use crate::utils::user_data::UserdataGetter;

/// Delegate type for handling xdg decoration events.
#[derive(Debug)]
pub struct XdgDecorationState {
    global: GlobalId,
}

/// Data associated with a XdgDecorationManager global.
#[allow(missing_debug_implementations)]
pub struct XdgDecorationManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl XdgDecorationState {
    /// Creates a new delegate type for handling xdg decoration events.
    ///
    /// A global id is also returned to allow destroying the global in the future.
    pub fn new<D>(display: &DisplayHandle) -> XdgDecorationState
    where
        D: XdgDecorationHandler,
    {
        Self::new_with_filter::<D, _>(display, |_| true)
    }

    /// Creates a new delegate type for handling xdg decoration events with a filter.
    ///
    /// Filters can be used to limit visibility of a global to certain clients.
    ///
    /// A global id is also returned to allow destroying the global in the future.
    pub fn new_with_filter<D, F>(display: &DisplayHandle, filter: F) -> XdgDecorationState
    where
        D: XdgDecorationHandler,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let data = XdgDecorationManagerGlobalData {
            filter: Box::new(filter),
        };
        let global = display
            .create_delegated_global::<D, zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, _, Self>(
                1, data,
            );

        XdgDecorationState { global }
    }

    /// Returns the xdg-decoration global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// Handler trait for xdg decoration events.
pub trait XdgDecorationHandler: XdgShellHandler + 'static {
    /// Notification the client supports server side decoration on the toplevel.
    fn new_decoration(&mut self, toplevel: ToplevelSurface);

    /// Notification the client prefers the provided decoration decoration mode on the toplevel.
    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode);

    /// Notification the client does not prefer a particular decoration mode on the toplevel.
    fn unset_mode(&mut self, toplevel: ToplevelSurface);
}

/// Macro to delegate implementation of the xdg decoration to [`XdgDecorationState`].
///
/// You must also implement [`XdgDecorationHandler`] to use this.
#[deprecated(note = "No longer needed, this is now NOP")]
#[macro_export]
macro_rules! delegate_xdg_decoration {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {};
}

pub(super) fn send_decoration_configure(id: &ZxdgToplevelDecorationV1, mode: Mode) {
    id.configure(mode)
}

impl<D> GlobalDispatch<ZxdgDecorationManagerV1, XdgDecorationManagerGlobalData, D> for XdgDecorationState
where
    D: XdgDecorationHandler,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZxdgDecorationManagerV1>,
        _: &XdgDecorationManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init_delegated::<_, _, Self>(resource, ());
    }

    fn can_view(client: Client, global_data: &XdgDecorationManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ZxdgDecorationManagerV1, (), D> for XdgDecorationState
where
    D: XdgDecorationHandler,
{
    fn request(
        state: &mut D,
        _: &Client,
        resource: &ZxdgDecorationManagerV1,
        request: zxdg_decoration_manager_v1::Request,
        _: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use zxdg_decoration_manager_v1::Request;

        match request {
            Request::GetToplevelDecoration { id, toplevel } => {
                let data = toplevel.user_data().unwrap();

                let mut decoration_guard = data.decoration.lock().unwrap();

                if decoration_guard.is_some() {
                    resource.post_error(
                        zxdg_toplevel_decoration_v1::Error::AlreadyConstructed,
                        "toplevel decoration is already constructed",
                    );
                    return;
                }

                let toplevel = state.xdg_shell_state().get_toplevel(&toplevel).unwrap();
                let toplevel_decoration = data_init.init_delegated::<_, _, Self>(id, toplevel.clone());

                *decoration_guard = Some(toplevel_decoration);
                drop(decoration_guard);

                state.new_decoration(toplevel);
            }

            Request::Destroy => {}

            _ => unreachable!(),
        }
    }
}

// zxdg_toplevel_decoration_v1

impl<D> Dispatch<ZxdgToplevelDecorationV1, ToplevelSurface, D> for XdgDecorationState
where
    D: XdgDecorationHandler,
{
    fn request(
        state: &mut D,
        _: &Client,
        _: &ZxdgToplevelDecorationV1,
        request: zxdg_toplevel_decoration_v1::Request,
        data: &ToplevelSurface,
        _dh: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        use zxdg_toplevel_decoration_v1::Request;

        match request {
            Request::SetMode { mode } => {
                if let WEnum::Value(mode) = mode {
                    state.request_mode(data.clone(), mode);
                }
            }

            Request::UnsetMode => {
                state.unset_mode(data.clone());
            }

            Request::Destroy => {
                if let Some(data) = data.xdg_toplevel().user_data() {
                    data.decoration.lock().unwrap().take();
                }
            }

            _ => unreachable!(),
        }
    }
}
