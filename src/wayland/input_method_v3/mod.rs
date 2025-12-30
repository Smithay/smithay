//! Utilities for input method support
//!
//! This module provides you with utilities to handle input methods,
//! it must be used in conjunction with the text input module to work.
//!
//! ```
//! use smithay::{
//!     delegate_seat, delegate_input_method_manager_v3, delegate_text_input_manager,
//! #   delegate_compositor,
//! };
//! use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
//! # use smithay::wayland::compositor::{CompositorHandler, CompositorState, CompositorClientState};
//! use smithay::wayland::input_method_v3::{InputMethodManagerState, InputMethodHandler, PopupSurface, PositionerState};
//! use smithay::wayland::text_input::TextInputManagerState;
//! use smithay::reexports::wayland_server::{Display, protocol::wl_surface::WlSurface};
//! # use smithay::reexports::wayland_server::Client;
//! use smithay::utils::{Rectangle, Logical};
//!
//! # struct State { seat_state: SeatState<Self> };
//!
//! delegate_seat!(State);
//! # delegate_compositor!(State);
//!
//! impl InputMethodHandler for State {
//!     fn new_popup(&mut self, surface: PopupSurface) {}
//!     fn dismiss_popup(&mut self, surface: PopupSurface) {}
//!     fn popup_repositioned(&mut self, surface: PopupSurface) {}
//!     fn popup_geometry(&self, _: &WlSurface, _: &Rectangle<i32, Logical>, _: &PositionerState) -> smithay::utils::Rectangle<i32, smithay::utils::Logical> {
//!         Rectangle::default()
//!     }
//!     fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, Logical> {
//!         Rectangle::default()
//!     }
//! }
//!
//! // Delegate input method handling for State to InputMethodManagerState.
//! delegate_input_method_manager_v3!(State);
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
//!     type TouchFocus = WlSurface;
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//!     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) { unimplemented!() }
//!     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) { unimplemented!() }
//! }
//!
//! # impl CompositorHandler for State {
//! #     fn compositor_state(&mut self) -> &mut CompositorState { unimplemented!() }
//! #     fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState { unimplemented!() }
//! #     fn commit(&mut self, surface: &WlSurface) {}
//! # }
//!
//! // Add the seat state to your state and create manager globals
//! InputMethodManagerState::new::<State, _>(&display_handle, |_client| true);
//!
//! // Add text input capabilities, needed for the input method to work
//! delegate_text_input_manager!(State);
//! TextInputManagerState::new::<State>(&display_handle);
//!
//! ```

use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle,
    GlobalDispatch, New,
};

use wayland_protocols_experimental::input_method::v1::{
    server::xx_input_popup_positioner_v1::XxInputPopupPositionerV1,
    server::{
        xx_input_method_manager_v2::{self, XxInputMethodManagerV2},
        xx_input_method_v1::XxInputMethodV1,
    },
};

use crate::{
    input::{Seat, SeatHandler},
    utils::{Logical, Rectangle, Serial},
};

pub use input_method_handle::{InputMethodHandle, InputMethodUserData};

use super::text_input::TextInputHandle;

const MANAGER_VERSION: u32 = 2;

/// The role of the input method popup.
pub const INPUT_POPUP_SURFACE_ROLE: &str = "zwp_input_popup_surface_v3";

mod configure_tracker;
mod input_method_handle;
mod input_method_popup_surface;
mod positioner;

pub use input_method_popup_surface::{
    InputMethodPopupSurfaceUserData, PopupParent, PopupSurface, PopupSurfaceState,
};
pub use positioner::{PositionerState, PositionerUserData};

/// Adds input method popup to compositor state
pub trait InputMethodHandler {
    /// Add a popup surface to compositor state.
    fn new_popup(&mut self, surface: PopupSurface);

    /// Dismiss a popup surface from the compositor state.
    fn dismiss_popup(&mut self, surface: PopupSurface);

    /// Popup location has changed.
    ///
    /// This gets called after calculating and applying the new geometry but before input_method.done is sent.
    fn popup_repositioned(&mut self, surface: PopupSurface);

    /// Returns the position of the popup, given the cursor rectangle expressed in position relative to surface.
    /// This may be called while locks on some input-method objects are held.
    fn popup_geometry(
        &self,
        parent: &WlSurface,
        cursor: &Rectangle<i32, Logical>,
        positioner: &PositionerState,
    ) -> Rectangle<i32, Logical>;

    /// Sets the parent location so the popup surface can be placed correctly
    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, Logical>;

    /// Copied from wl_layer_surface.
    /// What is this for? What arguments make sense?
    fn popup_ack_configure(
        &mut self,
        _surface: &WlSurface,
        _serial: Serial,
        _client_state: PopupSurfaceState,
    ) {
        // the compositor doesn't need to implement this if it doesn't have a use for it
    }
}

/// Extends [Seat] with input method functionality
pub trait InputMethodSeat {
    /// Get an input method associated with this seat
    fn input_method_v3(&self) -> &InputMethodHandle;
}

impl<D: SeatHandler + 'static> InputMethodSeat for Seat<D> {
    fn input_method_v3(&self) -> &InputMethodHandle {
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
        D: GlobalDispatch<XxInputMethodManagerV2, InputMethodManagerGlobalData>,
        D: Dispatch<XxInputMethodManagerV2, ()>,
        D: Dispatch<XxInputMethodV1, InputMethodUserData<D>>,
        D: SeatHandler,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let data = InputMethodManagerGlobalData {
            filter: Box::new(filter),
        };
        let global = display.create_global::<D, XxInputMethodManagerV2, _>(MANAGER_VERSION, data);

        Self { global }
    }

    /// Get the id of manager global
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<XxInputMethodManagerV2, InputMethodManagerGlobalData, D> for InputMethodManagerState
where
    D: GlobalDispatch<XxInputMethodManagerV2, InputMethodManagerGlobalData>,
    D: Dispatch<XxInputMethodManagerV2, ()>,
    D: Dispatch<XxInputMethodV1, InputMethodUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<XxInputMethodManagerV2>,
        _: &InputMethodManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &InputMethodManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<XxInputMethodManagerV2, (), D> for InputMethodManagerState
where
    D: Dispatch<XxInputMethodManagerV2, ()>,
    D: Dispatch<XxInputMethodV1, InputMethodUserData<D>>,
    D: Dispatch<XxInputPopupPositionerV1, PositionerUserData>,
    D: SeatHandler + InputMethodHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _: &XxInputMethodManagerV2,
        request: xx_input_method_manager_v2::Request,
        _: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xx_input_method_manager_v2::Request::GetInputMethod { seat, input_method } => {
                let seat = Seat::<D>::from_resource(&seat).unwrap();

                let user_data = seat.user_data();
                user_data.insert_if_missing(TextInputHandle::default);
                user_data.insert_if_missing(InputMethodHandle::default);
                let handle = user_data.get::<InputMethodHandle>().unwrap();
                let text_input_handle = user_data.get::<TextInputHandle>().unwrap();
                text_input_handle.with_focused_text_input(|ti, surface| {
                    ti.enter(surface);
                });
                let instance = data_init.init(
                    input_method,
                    InputMethodUserData {
                        handle: handle.clone(),
                        text_input_handle: text_input_handle.clone(),
                        dismiss_popup: D::dismiss_popup,
                        popup_geometry: D::popup_geometry,
                        popup_repositioned: D::popup_repositioned,
                    },
                );
                handle.add_instance(&instance);
            }
            xx_input_method_manager_v2::Request::GetPositioner { id } => {
                data_init.init(id, PositionerUserData::default());
            }
            xx_input_method_manager_v2::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_input_method_manager_v3 {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_experimental::input_method::v1::server::xx_input_method_manager_v2::XxInputMethodManagerV2:
            $crate::wayland::input_method_v3::InputMethodManagerGlobalData
        ] => $crate::wayland::input_method_v3::InputMethodManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_experimental::input_method::v1::server::xx_input_method_manager_v2::XxInputMethodManagerV2: ()
        ] => $crate::wayland::input_method_v3::InputMethodManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_experimental::input_method::v1::server::xx_input_method_v1::XxInputMethodV1:
            $crate::wayland::input_method_v3::InputMethodUserData<Self>
        ] => $crate::wayland::input_method_v3::InputMethodManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_experimental::input_method::v1::server::xx_input_popup_surface_v2::XxInputPopupSurfaceV2:
            $crate::wayland::input_method_v3::InputMethodPopupSurfaceUserData
        ] => $crate::wayland::input_method_v3::InputMethodManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_experimental::input_method::v1::server::xx_input_popup_positioner_v1::XxInputPopupPositionerV1:
            $crate::wayland::input_method_v3::PositionerUserData
        ] => $crate::wayland::input_method_v3::InputMethodManagerState);
    };
}
