//! XDG Dialog Windows
//!
//! This interface allows a compositor to announce support for xdg-dialog toplevel hints, eg. modal hint.
//!
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use smithay::{delegate_xdg_dialog, delegate_xdg_shell};
//! use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellHandler};
//! # use smithay::utils::Serial;
//! # use smithay::wayland::shell::xdg::{XdgShellState, PopupSurface, PositionerState};
//! # use smithay::reexports::wayland_server::protocol::{wl_seat, wl_surface};
//! use smithay::wayland::shell::xdg::dialog::{XdgDialogState, XdgDialogHandler};
//!
//! # struct State { dialog_state: XdgDialogState, seat_state: SeatState<Self> }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! // Create a dialog state
//! let dialog_state = XdgDialogState::new::<State>(
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
//! impl XdgDialogHandler for State {
//!     fn modal_changed(&mut self, toplevel: ToplevelSurface, is_modal: bool) { /* ... */ }
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
//! delegate_xdg_shell!(State);
//! delegate_xdg_dialog!(State);
//!
//! // You are ready to go!  

use wayland_protocols::xdg::dialog::v1::server::{
    xdg_dialog_v1::{self, XdgDialogV1},
    xdg_wm_dialog_v1::{self, XdgWmDialogV1},
};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle,
    GlobalDispatch, New, Resource,
};

use super::{ToplevelSurface, XdgShellHandler};
use crate::wayland::{
    compositor,
    shell::xdg::{XdgShellSurfaceUserData, XdgToplevelSurfaceData},
};

/// Delegate type for handling xdg dialog events.
#[derive(Debug)]
pub struct XdgDialogState {
    global: GlobalId,
}

impl XdgDialogState {
    /// Creates a new delegate type for handling xdg dialog events.
    ///
    /// A global id is also returned to allow destroying the global in the future.
    pub fn new<D>(display: &DisplayHandle) -> XdgDialogState
    where
        D: GlobalDispatch<XdgWmDialogV1, ()> + Dispatch<XdgWmDialogV1, ()> + 'static,
    {
        let global = display.create_global::<D, XdgWmDialogV1, _>(1, ());
        XdgDialogState { global }
    }

    /// Returns the xdg-wm-dialog global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// Handler trait for xdg dialog events.
pub trait XdgDialogHandler:
    XdgShellHandler
    + GlobalDispatch<XdgWmDialogV1, ()>
    + Dispatch<XdgWmDialogV1, ()>
    + Dispatch<XdgDialogV1, ToplevelSurface>
{
    /// Does client want to be presented as a modal dialog
    fn modal_changed(&mut self, toplevel: ToplevelSurface, is_modal: bool) {
        let _ = toplevel;
        let _ = is_modal;
    }
}

/// Macro to delegate implementation of the xdg dialog to [`XdgDialogState`].
///
/// You must also implement [`XdgDialogHandler`] to use this.
#[macro_export]
macro_rules! delegate_xdg_dialog {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::dialog::v1::server::xdg_wm_dialog_v1::XdgWmDialogV1: ()
        ] => $crate::wayland::shell::xdg::dialog::XdgDialogState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::dialog::v1::server::xdg_wm_dialog_v1::XdgWmDialogV1: ()
        ] => $crate::wayland::shell::xdg::dialog::XdgDialogState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::dialog::v1::server::xdg_dialog_v1::XdgDialogV1: $crate::wayland::shell::xdg::ToplevelSurface
        ] => $crate::wayland::shell::xdg::dialog::XdgDialogState);
    };
}

// xdg_wm_dialog_v1

impl<D: XdgDialogHandler> GlobalDispatch<XdgWmDialogV1, (), D> for XdgDialogState {
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<XdgWmDialogV1>,
        _: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D: XdgDialogHandler> Dispatch<XdgWmDialogV1, (), D> for XdgDialogState {
    fn request(
        state: &mut D,
        _: &Client,
        resource: &XdgWmDialogV1,
        request: xdg_wm_dialog_v1::Request,
        _: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use xdg_wm_dialog_v1::Request;

        match request {
            Request::GetXdgDialog { id, toplevel } => {
                let data = toplevel.data::<XdgShellSurfaceUserData>().unwrap();

                let mut dialog_guard = data.dialog.lock().unwrap();

                if dialog_guard.is_some() {
                    resource.post_error(
                        xdg_wm_dialog_v1::Error::AlreadyUsed,
                        "toplevel dialog is already constructed",
                    );
                    return;
                }

                let toplevel = state.xdg_shell_state().get_toplevel(&toplevel).unwrap();
                let toplevel_dialog = data_init.init(id, toplevel.clone());

                *dialog_guard = Some(toplevel_dialog);
                drop(dialog_guard);
            }

            Request::Destroy => {}

            _ => unreachable!(),
        }
    }
}

// xdg_dialog_v1

impl<D: XdgDialogHandler> Dispatch<XdgDialogV1, ToplevelSurface, D> for XdgDialogState {
    fn request(
        state: &mut D,
        _: &Client,
        _: &XdgDialogV1,
        request: xdg_dialog_v1::Request,
        toplevel: &ToplevelSurface,
        _dh: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        use xdg_dialog_v1::Request;

        match request {
            Request::SetModal => {
                if set_modal(toplevel.wl_surface()) {
                    state.modal_changed(toplevel.clone(), true);
                }
            }

            Request::UnsetModal => {
                if unset_modal(toplevel.wl_surface()) {
                    state.modal_changed(toplevel.clone(), false);
                }
            }

            Request::Destroy => {
                if unset_modal(toplevel.wl_surface()) {
                    state.modal_changed(toplevel.clone(), false);
                }

                if let Some(data) = toplevel.xdg_toplevel().data::<XdgShellSurfaceUserData>() {
                    data.dialog.lock().unwrap().take();
                }
            }

            _ => unreachable!(),
        }
    }
}

/// Returns true if changed
fn set_modal(wl_surface: &WlSurface) -> bool {
    compositor::with_states(wl_surface, |states| {
        let role = &mut states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .unwrap()
            .lock()
            .unwrap();

        if role.modal {
            false
        } else {
            role.modal = true;
            true
        }
    })
}

/// Returns true if changed
fn unset_modal(wl_surface: &WlSurface) -> bool {
    compositor::with_states(wl_surface, |states| {
        let role = &mut states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .unwrap()
            .lock()
            .unwrap();

        if role.modal {
            role.modal = false;
            true
        } else {
            false
        }
    })
}
