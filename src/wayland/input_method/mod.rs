//! Utilities for input method support
//!
//! This module provides you with utilities to handle input methods,
//! it must be used in conjunction with the text input module to work.
//!
//! ```
//! # extern crate wayland_server;
//! use smithay::{
//!     delegate_seat, delegate_tablet_manager, delegate_input_method_manager,
//!     delegate_text_input_manager,
//! };
//! use smithay::wayland::seat::{Seat, SeatState, SeatHandler, XkbConfig};
//! use smithay::wayland::input_method::{
//!     InputMethodManagerState,
//!     InputMethodSeat
//! };
//! use smithay::wayland::text_input::TextInputManagerState;
//!
//! # struct State { seat_state: SeatState<Self> };
//!
//! delegate_seat!(State);
//! // Delegate input method handling for State to InputMethodManagerState.
//! delegate_input_method_manager!(State);
//! delegate_text_input_manager!(State);
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! let seat_state = SeatState::<State>::new();
//!
//! // implement the required traits
//! impl SeatHandler for State {
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//! }
//!
//! // Add the seat state to your state and create manager globals
//! InputMethodManagerState::new::<State>(&display_handle);
//! // Add text input capabilities, needed for the input method to work
//! TextInputManagerState::new::<State>(&display_handle);
//!
//! // create the seat
//! let seat = Seat::<State>::new(
//!     &display_handle,          // the display
//!     "seat-0",                 // the name of the seat, will be advertized to clients
//!     None                      // insert a logger here
//! );
//!
//! seat.add_input_method(XkbConfig::default(), 200, 25);
//! // Add input method capabilities to a seat
//!
//! ```
//! ### Run usage
//!
//! Once the input method and text input cabailities have been added to a seat,
//! use the [`seat.input_method().set_point`] function to set the top left point
//! of a focused surface. This is used to calculate the popup surface location.
//!
//! Then use the [`seat.input_method().coordinates`] and [`seat.input_method().with_surface`]
//! functions to draw the popup surface.
//!

use wayland_server::{backend::GlobalId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};

use wayland_protocols_misc::zwp_input_method_v2::server::{
    zwp_input_method_manager_v2::{self, ZwpInputMethodManagerV2},
    zwp_input_method_v2::ZwpInputMethodV2,
};

use crate::wayland::seat::Seat;

pub use input_method_handle::{InputMethodHandle, InputMethodUserData};
pub use input_method_keyboard_grab::InputMethodKeyboardUserData;
pub use input_method_popup_surface::InputMethodPopupSurfaceUserData;

use super::{seat::XkbConfig, text_input::TextInputHandle};

const MANAGER_VERSION: u32 = 1;

mod input_method_handle;
mod input_method_keyboard_grab;
mod input_method_popup_surface;

/// Extends [Seat] with input method functionality
pub trait InputMethodSeat {
    /// Add an input method to this seat, and configures the associated keyboard.
    /// Input methods need different keyboard languages for different input methods.
    /// E.g a pinyin user will want to use their native keyboard layout, but a
    /// zhuyin user will always want a taiwanese keyboard layout.
    fn add_input_method(&self, xkb_config: XkbConfig<'_>, repeat_delay: i32, repeat_rate: i32);

    /// Get an input method associated with this seat
    fn input_method(&self) -> InputMethodHandle;
}

impl<D: 'static> InputMethodSeat for Seat<D> {
    fn add_input_method(&self, xkb_config: XkbConfig<'_>, repeat_delay: i32, repeat_rate: i32) {
        let user_data = self.user_data();
        user_data.insert_if_missing(InputMethodHandle::default);
        let input_method = user_data.get::<InputMethodHandle>().unwrap().clone();
        input_method.configure_keyboard(xkb_config, repeat_delay, repeat_rate);
    }

    fn input_method(&self) -> InputMethodHandle {
        let user_data = self.user_data();
        user_data.get::<InputMethodHandle>().unwrap().clone()
    }
}

/// State of wp misc input method protocol
#[derive(Debug)]
pub struct InputMethodManagerState {
    global: GlobalId,
}

impl InputMethodManagerState {
    /// Initialize a text input manager global.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwpInputMethodManagerV2, ()>,
        D: Dispatch<ZwpInputMethodManagerV2, ()>,
        D: Dispatch<ZwpInputMethodV2, InputMethodUserData>,
        D: 'static,
    {
        let global = display.create_global::<D, ZwpInputMethodManagerV2, _>(MANAGER_VERSION, ());

        Self { global }
    }

    /// Get the id of ZwpTextInputManagerV3 global
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<ZwpInputMethodManagerV2, (), D> for InputMethodManagerState
where
    D: GlobalDispatch<ZwpInputMethodManagerV2, ()>,
    D: Dispatch<ZwpInputMethodManagerV2, ()>,
    D: Dispatch<ZwpInputMethodV2, InputMethodUserData>,
    D: 'static,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpInputMethodManagerV2>,
        _: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<ZwpInputMethodManagerV2, (), D> for InputMethodManagerState
where
    D: Dispatch<ZwpInputMethodManagerV2, ()>,
    D: Dispatch<ZwpInputMethodV2, InputMethodUserData>,
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
                let handle = user_data.get::<InputMethodHandle>().unwrap();
                let text_input_handle = user_data.get::<TextInputHandle>().unwrap();
                let keyboard_handle = seat.get_keyboard().unwrap();
                let instance = data_init.init(
                    input_method,
                    InputMethodUserData {
                        handle: handle.clone(),
                        text_input_handle: text_input_handle.clone(),
                        keyboard_handle,
                    },
                );
                handle.add_instance::<InputMethodHandle>(&instance);
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
            $crate::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_manager_v2::ZwpInputMethodManagerV2: ()
        ] => $crate::wayland::input_method::InputMethodManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_manager_v2::ZwpInputMethodManagerV2: ()
        ] => $crate::wayland::input_method::InputMethodManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_v2::ZwpInputMethodV2: $crate::wayland::input_method::InputMethodUserData
        ] => $crate::wayland::input_method::InputMethodManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2: $crate::wayland::input_method::InputMethodKeyboardUserData
        ] => $crate::wayland::input_method::InputMethodManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2: $crate::wayland::input_method::InputMethodPopupSurfaceUserData
        ] => $crate::wayland::input_method::InputMethodManagerState);
    };
}
