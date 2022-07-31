//! Utilities for text input support
//!
//! This module provides you with utilities to handle text input surfaces,
//! it must be used in conjunction with the input method module to work.
//!
//! ```
//! # extern crate wayland_server;
//! use smithay::{
//!     delegate_seat, delegate_tablet_manager, delegate_input_method_manager,
//!     delegate_text_input_manager,
//! };
//! use smithay::wayland::seat::{Seat, SeatState, SeatHandler, XkbConfig};
//! use smithay::wayland::input_method_manager::{
//!     InputMethodManagerState,
//!     InputMethodSeatTrait
//! };
//! use smithay::wayland::text_input_manager::{
//!     TextInputManagerState,
//!     TextInputSeatTrait
//! };
//!
//! # struct State { seat_state: SeatState<Self> };
//!
//! delegate_seat!(State);
//! delegate_input_method_manager!(State);
//! // Delegate text input handling for State to TextInputManagerState.
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
//! TextInputManagerState::new::<State>(&display_handle);
//!
//! // create the seat
//! let seat = Seat::<State>::new(
//!     &display_handle,          // the display
//!     "seat-0",                 // the name of the seat, will be advertized to clients
//!     None                      // insert a logger here
//! );
//!
//! seat.text_input();
//! // Add text input capabilities to a seat
//! seat.add_input_method(XkbConfig::default(), 200, 25);
//! // Add input method capabilities to a seat, , needed for text input to work
//!
//! ```
//!

use wayland_protocols::wp::text_input::zv3::server::{
    zwp_text_input_manager_v3::{self, ZwpTextInputManagerV3},
    zwp_text_input_v3::ZwpTextInputV3,
};
use wayland_server::{backend::GlobalId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};

use crate::wayland::seat::Seat;

pub use text_input::TextInputHandle;
pub use text_input::TextInputUserData;

use super::input_method_manager::InputMethodHandle;

const MANAGER_VERSION: u32 = 1;

mod text_input;

/// Extends [Seat] with text input functionality
pub trait TextInputSeatTrait {
    /// Get text input associated with this seat
    fn text_input(&self) -> TextInputHandle;
}

impl<D: 'static> TextInputSeatTrait for Seat<D> {
    fn text_input(&self) -> TextInputHandle {
        let user_data = self.user_data();
        user_data.insert_if_missing(TextInputHandle::default);
        user_data.get::<TextInputHandle>().unwrap().clone()
    }
}

/// State of wp text input protocol
#[derive(Debug)]
pub struct TextInputManagerState {
    global: GlobalId,
}

impl TextInputManagerState {
    /// Initialize a text input manager global.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwpTextInputManagerV3, ()>,
        D: Dispatch<ZwpTextInputManagerV3, ()>,
        D: Dispatch<ZwpTextInputV3, TextInputUserData>,
        D: 'static,
    {
        let global = display.create_global::<D, ZwpTextInputManagerV3, _>(MANAGER_VERSION, ());

        Self { global }
    }

    /// Get the id of ZwpTextInputManagerV3 global
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<ZwpTextInputManagerV3, (), D> for TextInputManagerState
where
    D: GlobalDispatch<ZwpTextInputManagerV3, ()>,
    D: Dispatch<ZwpTextInputManagerV3, ()>,
    D: Dispatch<ZwpTextInputV3, TextInputUserData>,
    D: 'static,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpTextInputManagerV3>,
        _: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<ZwpTextInputManagerV3, (), D> for TextInputManagerState
where
    D: Dispatch<ZwpTextInputManagerV3, ()>,
    D: Dispatch<ZwpTextInputV3, TextInputUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &ZwpTextInputManagerV3,
        request: zwp_text_input_manager_v3::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_text_input_manager_v3::Request::GetTextInput { id, seat } => {
                let seat = Seat::<D>::from_resource(&seat).unwrap();

                let user_data = seat.user_data();
                user_data.insert_if_missing(InputMethodHandle::default);

                let handle = user_data.get::<TextInputHandle>().unwrap();
                let input_method_handle = user_data.get::<InputMethodHandle>().unwrap();
                let instance = data_init.init(
                    id,
                    TextInputUserData {
                        handle: handle.clone(),
                        input_method_handle: input_method_handle.clone(),
                    },
                );

                handle.add_instance::<D>(&instance);
            }
            zwp_text_input_manager_v3::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_text_input_manager {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_manager_v3::ZwpTextInputManagerV3: ()
        ] => $crate::wayland::text_input_manager::TextInputManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_manager_v3::ZwpTextInputManagerV3: ()
        ] => $crate::wayland::text_input_manager::TextInputManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_v3::ZwpTextInputV3: $crate::wayland::text_input_manager::TextInputUserData
        ] => $crate::wayland::text_input_manager::TextInputManagerState);
    };
}
