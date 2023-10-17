//! Utilities for input method support
//!
//! This module provides you with utilities to handle input methods,
//! it must be used in conjunction with the text input module to work.
//!
//! ```
//! use smithay::{
//!     delegate_seat, delegate_input_method_manager, delegate_text_input_manager,
//! };
//! use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
//! use smithay::wayland::input_method::{InputMethodManagerState, InputMethodHandler, PopupSurface};
//! use smithay::wayland::text_input::TextInputManagerState;
//! use smithay::reexports::wayland_server::{Display, protocol::wl_surface::WlSurface};
//! use smithay::utils::{Rectangle, Logical};
//!
//! # struct State { seat_state: SeatState<Self> };
//!
//! delegate_seat!(State);
//!
//! impl InputMethodHandler for State {
//!     fn new_popup(&mut self, surface: PopupSurface) {}
//!     fn dismiss_popup(&mut self, surface: PopupSurface) {}
//!     fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, Logical> {
//!         Rectangle::default()
//!     }
//! }
//!
//! // Delegate input method handling for State to InputMethodManagerState.
//! delegate_input_method_manager!(State);
//!
//! delegate_text_input_manager!(State);
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! let mut seat_state = SeatState::<State>::new();
//!
//! // implement the required traits
//! impl SeatHandler for State {
//!     type KeyboardFocus = WlSurface;
//!     type PointerFocus = WlSurface;
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//!     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) { unimplemented!() }
//!     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
//! }
//!
//! // Add the seat state to your state and create manager globals
//! InputMethodManagerState::new::<State, _>(&display_handle, |_client| true);
//! // Add text input capabilities, needed for the input method to work
//! TextInputManagerState::new::<State>(&display_handle);
//!
//! ```

use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle,
    GlobalDispatch, New,
};

use wayland_protocols_misc::zwp_input_method_v2::server::{
    zwp_input_method_manager_v2::{self, ZwpInputMethodManagerV2},
    zwp_input_method_v2::ZwpInputMethodV2,
};

use crate::{
    input::{Seat, SeatHandler},
    utils::{Logical, Rectangle},
};

pub use input_method_handle::{InputMethodHandle, InputMethodUserData};
pub use input_method_keyboard_grab::InputMethodKeyboardUserData;
pub use input_method_popup_surface::InputMethodPopupSurfaceUserData;

use super::text_input::TextInputHandle;

const MANAGER_VERSION: u32 = 1;

/// The role of the input method popup.
pub const INPUT_POPUP_SURFACE_ROLE: &str = "zwp_input_popup_surface_v2";

mod input_method_handle;
mod input_method_keyboard_grab;
mod input_method_popup_surface;
pub use input_method_popup_surface::{PopupParent, PopupSurface};

/// Adds input method popup to compositor state
pub trait InputMethodHandler {
    /// Add a popup surface to compositor state.
    fn new_popup(&mut self, surface: PopupSurface);

    /// Dismiss a popup surface from the compositor state.
    fn dismiss_popup(&mut self, surface: PopupSurface);

    /// Sets the parent location so the popup surface can be placed correctly
    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, Logical>;
}

/// Extends [Seat] with input method functionality
pub trait InputMethodSeat {
    /// Get an input method associated with this seat
    fn input_method(&self) -> &InputMethodHandle;
}

impl<D: SeatHandler + 'static> InputMethodSeat for Seat<D> {
    fn input_method(&self) -> &InputMethodHandle {
        let user_data = self.user_data();
        user_data.insert_if_missing(InputMethodHandle::default);
        user_data.get::<InputMethodHandle>().unwrap()
    }
}

/// Data associated with a InputMethodManager global.
#[allow(missing_debug_implementations)]
pub struct InputMethodManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

/// State of wp misc input method protocol
#[derive(Debug)]
pub struct InputMethodManagerState {
    global: GlobalId,
}

impl InputMethodManagerState {
    /// Initialize a text input manager global.
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ZwpInputMethodManagerV2, InputMethodManagerGlobalData>,
        D: Dispatch<ZwpInputMethodManagerV2, ()>,
        D: Dispatch<ZwpInputMethodV2, InputMethodUserData<D>>,
        D: SeatHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let data = InputMethodManagerGlobalData {
            filter: Box::new(filter),
        };
        let global = display.create_global::<D, ZwpInputMethodManagerV2, _>(MANAGER_VERSION, data);

        Self { global }
    }

    /// Get the id of ZwpTextInputManagerV3 global
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<ZwpInputMethodManagerV2, InputMethodManagerGlobalData, D> for InputMethodManagerState
where
    D: GlobalDispatch<ZwpInputMethodManagerV2, InputMethodManagerGlobalData>,
    D: Dispatch<ZwpInputMethodManagerV2, ()>,
    D: Dispatch<ZwpInputMethodV2, InputMethodUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpInputMethodManagerV2>,
        _: &InputMethodManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &InputMethodManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ZwpInputMethodManagerV2, (), D> for InputMethodManagerState
where
    D: Dispatch<ZwpInputMethodManagerV2, ()>,
    D: Dispatch<ZwpInputMethodV2, InputMethodUserData<D>>,
    D: SeatHandler + InputMethodHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _: &ZwpInputMethodManagerV2,
        request: zwp_input_method_manager_v2::Request,
        _: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_input_method_manager_v2::Request::GetInputMethod { seat, input_method } => {
                let seat = Seat::<D>::from_resource(&seat).unwrap();

                let user_data = seat.user_data();
                user_data.insert_if_missing(TextInputHandle::default);
                user_data.insert_if_missing(InputMethodHandle::default);
                let handle = user_data.get::<InputMethodHandle>().unwrap();
                let text_input_handle = user_data.get::<TextInputHandle>().unwrap();
                text_input_handle.with_focused_text_input(|ti, surface| {
                    ti.enter(surface);
                });
                let keyboard_handle = seat.get_keyboard().unwrap();
                let instance = data_init.init(
                    input_method,
                    InputMethodUserData {
                        handle: handle.clone(),
                        text_input_handle: text_input_handle.clone(),
                        keyboard_handle,
                        popup_geometry_callback: D::parent_geometry,
                        new_popup: D::new_popup,
                        dismiss_popup: D::dismiss_popup,
                    },
                );
                handle.add_instance(&instance);
            }
            zwp_input_method_manager_v2::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_input_method_manager {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_manager_v2::ZwpInputMethodManagerV2:
            $crate::wayland::input_method::InputMethodManagerGlobalData
        ] => $crate::wayland::input_method::InputMethodManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_manager_v2::ZwpInputMethodManagerV2: ()
        ] => $crate::wayland::input_method::InputMethodManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_v2::ZwpInputMethodV2:
            $crate::wayland::input_method::InputMethodUserData<Self>
        ] => $crate::wayland::input_method::InputMethodManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2:
            $crate::wayland::input_method::InputMethodKeyboardUserData<Self>
        ] => $crate::wayland::input_method::InputMethodManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2:
            $crate::wayland::input_method::InputMethodPopupSurfaceUserData
        ] => $crate::wayland::input_method::InputMethodManagerState);
    };
}
