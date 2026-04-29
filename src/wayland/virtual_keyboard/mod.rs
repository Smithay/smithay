//! Utilities for virtual keyboard support
//!
//! This module provides you with utilities to handle virtual keyboard instances.
//! It can be used standalone to implement virtual keyboards or together with
//! an input method to pass through keys from the keyboard.
//!
//! ```
//! use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
//! # use smithay::wayland::compositor::{CompositorHandler, CompositorState, CompositorClientState};
//! use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;
//! use smithay::reexports::wayland_server::{Display, protocol::wl_surface::WlSurface};
//! # use smithay::reexports::wayland_server::Client;
//!
//! # struct State { seat_state: SeatState<Self> };
//!
//! smithay::delegate_dispatch2!(State);
//!
//! # let mut display = Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! let seat_state = SeatState::<State>::new();
//!
//! // implement the required traits
//! impl SeatHandler for State {
//!     type KeyboardFocus = WlSurface;
//!     type PointerFocus = WlSurface;
//!     type TouchFocus = WlSurface;
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//!     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) { unimplemented!() }
//!     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
//! }
//!
//! // Add the seat state to your state, create manager global and add client filter
//! // to avoid untrusted clients requesting a new keyboard
//! VirtualKeyboardManagerState::new::<State, _>(&display_handle, |_client| true);
//!
//! # impl CompositorHandler for State {
//! #     fn compositor_state(&mut self) -> &mut CompositorState { unimplemented!() }
//! #     fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState { unimplemented!() }
//! #     fn commit(&mut self, surface: &WlSurface) {}
//! # }
//! ```
//!

use wayland_protocols_misc::zwp_virtual_keyboard_v1::server::{
    zwp_virtual_keyboard_manager_v1::{self, ZwpVirtualKeyboardManagerV1},
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, backend::GlobalId};

use crate::{
    input::{Seat, SeatHandler},
    wayland::{GlobalData, seat::WaylandFocus},
};

use self::virtual_keyboard_handle::VirtualKeyboardHandle;

const MANAGER_VERSION: u32 = 1;

mod virtual_keyboard_handle;

pub use virtual_keyboard_handle::VirtualKeyboardUserData;

/// State of wp misc virtual keyboard protocol
#[derive(Debug)]
pub struct VirtualKeyboardManagerState {
    global: GlobalId,
}

/// Data associated with a VirtualKeyboardManager global.
#[allow(missing_debug_implementations)]
pub struct VirtualKeyboardManagerGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

fn create_global_with_filter<D, F>(display: &DisplayHandle, filter: F) -> GlobalId
where
    D: SeatHandler + 'static,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
    F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
{
    let data = VirtualKeyboardManagerGlobalData {
        filter: Box::new(filter),
    };

    display.create_global::<D, ZwpVirtualKeyboardManagerV1, _>(MANAGER_VERSION, data)
}

impl VirtualKeyboardManagerState {
    /// Initialize a virtual keyboard manager global.
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: SeatHandler + 'static,
        <D as SeatHandler>::KeyboardFocus: WaylandFocus,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global = create_global_with_filter::<D, F>(display, filter);

        Self { global }
    }

    /// Get the id of ZwpVirtualKeyboardManagerV1 global
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<ZwpVirtualKeyboardManagerV1, D> for VirtualKeyboardManagerGlobalData
where
    D: SeatHandler + 'static,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
{
    fn bind(
        &self,
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpVirtualKeyboardManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D> Dispatch<ZwpVirtualKeyboardManagerV1, D> for GlobalData
where
    D: SeatHandler + 'static,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &Client,
        _resource: &ZwpVirtualKeyboardManagerV1,
        request: zwp_virtual_keyboard_manager_v1::Request,
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_virtual_keyboard_manager_v1::Request::CreateVirtualKeyboard { seat, id } => {
                let seat = Seat::<D>::from_resource(&seat).unwrap();
                let virtual_keyboard_handle = VirtualKeyboardHandle::default();
                data_init.init(
                    id,
                    VirtualKeyboardUserData {
                        handle: virtual_keyboard_handle,
                        seat: seat.clone(),
                    },
                );
            }
            _ => unreachable!(),
        }
    }
}
