//! Utilities for handling the xwayland keyboard grab protocol
//!
//! ```
//! use smithay::wayland::xwayland_keyboard_grab::{
//!     XWaylandKeyboardGrabHandler,
//!     XWaylandKeyboardGrabState
//! };
//! use smithay::delegate_xwayland_keyboard_grab;
//!
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the keyboard grab state:
//! XWaylandKeyboardGrabState::new::<State>(
//!     &display.handle(), // the display
//! );
//! #
//! # use smithay::input::{Seat, SeatHandler, SeatState, pointer::CursorImageStatus};
//! # use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
//! # impl SeatHandler for State {
//! #     type KeyboardFocus = WlSurface;
//! #     type PointerFocus = WlSurface;
//! #     type TouchFocus = WlSurface;
//! #     fn seat_state(&mut self) -> &mut SeatState<Self> { unimplemented!() }
//! #     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) { unimplemented!() }
//! #     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
//! # }
//!
//! impl XWaylandKeyboardGrabHandler for State {
//!     fn keyboard_focus_for_xsurface(&self, _: &WlSurface) -> Option<Self::KeyboardFocus> {
//!         // Return a `SeatHandler::KeyboardFocus` that the grab can use to set focus to the
//!         // XWayland surface corresponding to this `WlSurface`.
//!         todo!()
//!     }
//! }
//!
//! // implement Dispatch for the keyboard grab types
//! delegate_xwayland_keyboard_grab!(State);
//! ```

use wayland_protocols::xwayland::keyboard_grab::zv1::server::{
    zwp_xwayland_keyboard_grab_manager_v1::{self, ZwpXwaylandKeyboardGrabManagerV1},
    zwp_xwayland_keyboard_grab_v1::{self, ZwpXwaylandKeyboardGrabV1},
};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
    Resource,
};

use crate::{
    backend::input::{KeyState, Keycode},
    input::{
        keyboard::{self, KeyboardGrab, KeyboardInnerHandle},
        Seat, SeatHandler,
    },
    utils::{Serial, SERIAL_COUNTER},
    xwayland::XWaylandClientData,
};

const MANAGER_VERSION: u32 = 1;

/// Handler for xwayland keyboard grab protocol
pub trait XWaylandKeyboardGrabHandler: SeatHandler {
    /// XWayland client has requested a keyboard grab for `surface` on `seat`
    ///
    /// The default implementation calls `KeyboardHandle::set_grab` if `seat`
    /// has a keyboard.
    fn grab(&mut self, _surface: wl_surface::WlSurface, seat: Seat<Self>, grab: XWaylandKeyboardGrab<Self>) {
        if let Some(keyboard) = seat.get_keyboard() {
            keyboard.set_grab(self, grab, SERIAL_COUNTER.next_serial());
        }
    }

    /// Defines what `KeyboardFocus` an `XWaylandKeyboardGrab` can use to focus the
    /// `X11Surface` corresponding to `surface`.
    ///
    /// If this returns `None`, no grab is created.
    fn keyboard_focus_for_xsurface(&self, surface: &wl_surface::WlSurface) -> Option<Self::KeyboardFocus>;
}

/// A grab created by xwayland keyboard grab protocol.
///
/// This implements `KeyboardGrab`, and deactivates the grab when the corresponding
/// `ZwpXwaylandKeyboardGrabV1` is destroyed by the client.
#[derive(Debug)]
pub struct XWaylandKeyboardGrab<D: SeatHandler + 'static> {
    grab: ZwpXwaylandKeyboardGrabV1,
    start_data: keyboard::GrabStartData<D>,
}

impl<D: XWaylandKeyboardGrabHandler + 'static> XWaylandKeyboardGrab<D> {
    /// Get the `zwp_xwayland_keyboard_grab_v1` object that created the grab
    pub fn grab(&self) -> &ZwpXwaylandKeyboardGrabV1 {
        &self.grab
    }
}

impl<D: XWaylandKeyboardGrabHandler + 'static> Clone for XWaylandKeyboardGrab<D> {
    fn clone(&self) -> Self {
        Self {
            grab: self.grab.clone(),
            start_data: self.start_data.clone(),
        }
    }
}

impl<D: XWaylandKeyboardGrabHandler + 'static> KeyboardGrab<D> for XWaylandKeyboardGrab<D> {
    fn input(
        &mut self,
        data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        keycode: Keycode,
        state: KeyState,
        modifiers: Option<keyboard::ModifiersState>,
        serial: Serial,
        time: u32,
    ) {
        handle.set_focus(data, self.start_data.focus.clone(), serial);

        if !self.grab.is_alive() {
            handle.unset_grab(self, data, serial, false);
        }

        handle.input(data, keycode, state, modifiers, serial, time)
    }

    fn set_focus(
        &mut self,
        data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        focus: Option<<D as SeatHandler>::KeyboardFocus>,
        serial: Serial,
    ) {
        if !self.grab.is_alive() {
            handle.unset_grab(self, data, serial, false);
            handle.set_focus(data, focus, serial);
        }
    }

    fn start_data(&self) -> &keyboard::GrabStartData<D> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut D) {}
}

/// State of the xwayland keyboard grab global
#[derive(Debug)]
pub struct XWaylandKeyboardGrabState {
    global: GlobalId,
}

impl XWaylandKeyboardGrabState {
    /// Register new [ZwpXwaylandKeyboardGrabManagerV1] global
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwpXwaylandKeyboardGrabManagerV1, ()>,
        D: Dispatch<ZwpXwaylandKeyboardGrabManagerV1, ()>,
        D: Dispatch<ZwpXwaylandKeyboardGrabV1, ()>,
        D: 'static,
    {
        let global = display.create_global::<D, ZwpXwaylandKeyboardGrabManagerV1, _>(MANAGER_VERSION, ());

        Self { global }
    }

    /// [ZwpXwaylandKeyboardGrabV1] GlobalId getter
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<ZwpXwaylandKeyboardGrabManagerV1, (), D> for XWaylandKeyboardGrabState
where
    D: GlobalDispatch<ZwpXwaylandKeyboardGrabManagerV1, ()>
        + Dispatch<ZwpXwaylandKeyboardGrabManagerV1, ()>
        + 'static,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwpXwaylandKeyboardGrabManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _global_data: &()) -> bool {
        client.get_data::<XWaylandClientData>().is_some()
    }
}

impl<D> Dispatch<ZwpXwaylandKeyboardGrabManagerV1, (), D> for XWaylandKeyboardGrabState
where
    D: Dispatch<ZwpXwaylandKeyboardGrabManagerV1, ()> + 'static,
    D: Dispatch<ZwpXwaylandKeyboardGrabV1, ()> + 'static,
    D: XWaylandKeyboardGrabHandler,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _grab_manager: &ZwpXwaylandKeyboardGrabManagerV1,
        request: zwp_xwayland_keyboard_grab_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_xwayland_keyboard_grab_manager_v1::Request::GrabKeyboard { id, surface, seat } => {
                let grab = data_init.init(id, ());
                if let Some(focus) = state.keyboard_focus_for_xsurface(&surface) {
                    let grab = XWaylandKeyboardGrab {
                        grab,
                        start_data: keyboard::GrabStartData { focus: Some(focus) },
                    };
                    let seat = Seat::from_resource(&seat).unwrap();
                    state.grab(surface, seat, grab);
                }
            }
            zwp_xwayland_keyboard_grab_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<ZwpXwaylandKeyboardGrabV1, (), D> for XWaylandKeyboardGrabState
where
    D: Dispatch<ZwpXwaylandKeyboardGrabV1, ()> + 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _grab: &ZwpXwaylandKeyboardGrabV1,
        request: zwp_xwayland_keyboard_grab_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_xwayland_keyboard_grab_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

/// Macro to delegate implementation of the xwayland keyboard grab protocol
#[macro_export]
macro_rules! delegate_xwayland_keyboard_grab {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xwayland::keyboard_grab::zv1::server::zwp_xwayland_keyboard_grab_manager_v1::ZwpXwaylandKeyboardGrabManagerV1: ()
        ] => $crate::wayland::xwayland_keyboard_grab::XWaylandKeyboardGrabState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xwayland::keyboard_grab::zv1::server::zwp_xwayland_keyboard_grab_manager_v1::ZwpXwaylandKeyboardGrabManagerV1: ()
        ] => $crate::wayland::xwayland_keyboard_grab::XWaylandKeyboardGrabState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xwayland::keyboard_grab::zv1::server::zwp_xwayland_keyboard_grab_v1::ZwpXwaylandKeyboardGrabV1: ()
        ] => $crate::wayland::xwayland_keyboard_grab::XWaylandKeyboardGrabState);
    };
}
