//! Utilities for text input support
//!
//! This module provides you with utilities to handle text input surfaces,
//! it is usually used in conjunction with the input method module.
//!
//! ```
//! # extern crate wayland_server;
//! use smithay::{
//!     delegate_seat, delegate_tablet_manager, delegate_text_input_manager,
//! };
//! use smithay::wayland::seat::{Seat, SeatState, SeatHandler, XkbConfig};
//! use smithay::wayland::text_input::TextInputManagerState;
//!
//! # struct State { seat_state: SeatState<Self> };
//!
//! delegate_seat!(State);
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
//! // Add the seat state to your state and create manager global
//! TextInputManagerState::new::<State>(&display_handle);
//!
//! ```
//!

use wayland_protocols::wp::text_input::zv3::server::{
    zwp_text_input_manager_v3::{self, ZwpTextInputManagerV3},
    zwp_text_input_v3::ZwpTextInputV3,
};
use wayland_server::{backend::GlobalId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};

use crate::input::{Seat, SeatHandler};

pub use text_input_handle::TextInputHandle;
pub use text_input_handle::TextInputUserData;

use super::input_method::InputMethodHandle;

const MANAGER_VERSION: u32 = 1;

mod text_input_handle;

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
    D: SeatHandler,
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
                user_data.insert_if_missing(TextInputHandle::default);
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
        ] => $crate::wayland::text_input::TextInputManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_manager_v3::ZwpTextInputManagerV3: ()
        ] => $crate::wayland::text_input::TextInputManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_v3::ZwpTextInputV3: $crate::wayland::text_input::TextInputUserData
        ] => $crate::wayland::text_input::TextInputManagerState);
    };
}
