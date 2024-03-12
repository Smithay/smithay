//! Utilities for relative pointer support
//!
//! [PointerHandle::relative_motion] sends relative pointer events to any
//! [ZwpRelativePointerV1] objects created by the client.
//!
//! ```
//! extern crate wayland_server;
//! extern crate smithay;
//!
//! use smithay::wayland::relative_pointer::RelativePointerManagerState;
//! # use smithay::backend::input::KeyState;
//! # use smithay::input::{
//! #   pointer::{PointerTarget, AxisFrame, MotionEvent, ButtonEvent, RelativeMotionEvent,
//! #             GestureSwipeBeginEvent, GestureSwipeUpdateEvent, GestureSwipeEndEvent,
//! #             GesturePinchBeginEvent, GesturePinchUpdateEvent, GesturePinchEndEvent,
//! #             GestureHoldBeginEvent, GestureHoldEndEvent},
//! #   keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
//! #   touch::{DownEvent, UpEvent, MotionEvent as TouchMotionEvent, ShapeEvent, OrientationEvent, TouchTarget},
//! #   Seat, SeatHandler, SeatState,
//! # };
//! # use smithay::utils::{IsAlive, Serial};
//!
//! # #[derive(Debug, Clone, PartialEq)]
//! # struct Target;
//! # impl IsAlive for Target {
//! #   fn alive(&self) -> bool { true }
//! # }
//! # impl PointerTarget<State> for Target {
//! #   fn enter(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
//! #   fn motion(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {}
//! #   fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {}
//! #   fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {}
//! #   fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {}
//! #   fn frame(&self, seat: &Seat<State>, data: &mut State) {}
//! #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {}
//! #   fn gesture_swipe_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeBeginEvent) {}
//! #   fn gesture_swipe_update(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeUpdateEvent) {}
//! #   fn gesture_swipe_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeEndEvent) {}
//! #   fn gesture_pinch_begin(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchBeginEvent) {}
//! #   fn gesture_pinch_update(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchUpdateEvent) {}
//! #   fn gesture_pinch_end(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchEndEvent) {}
//! #   fn gesture_hold_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldBeginEvent) {}
//! #   fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {}
//! # }
//! # impl KeyboardTarget<State> for Target {
//! #   fn enter(&self, seat: &Seat<State>, data: &mut State, keys: Vec<KeysymHandle<'_>>, serial: Serial) {}
//! #   fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {}
//! #   fn key(
//! #       &self,
//! #       seat: &Seat<State>,
//! #       data: &mut State,
//! #       key: KeysymHandle<'_>,
//! #       state: KeyState,
//! #       serial: Serial,
//! #       time: u32,
//! #   ) {}
//! #   fn modifiers(&self, seat: &Seat<State>, data: &mut State, modifiers: ModifiersState, serial: Serial) {}
//! # }
//! # impl TouchTarget<State> for Target {
//! #   fn down(&self, seat: &Seat<State>, data: &mut State, event: &DownEvent, seq: Serial) {}
//! #   fn up(&self, seat: &Seat<State>, data: &mut State, event: &UpEvent, seq: Serial) {}
//! #   fn motion(&self, seat: &Seat<State>, data: &mut State, event: &TouchMotionEvent, seq: Serial) {}
//! #   fn frame(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {}
//! #   fn cancel(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {}
//! #   fn shape(&self, seat: &Seat<State>, data: &mut State, event: &ShapeEvent, seq: Serial) {}
//! #   fn orientation(&self, seat: &Seat<State>, data: &mut State, event: &OrientationEvent, seq: Serial) {}
//! # }
//! # struct State {
//! #     seat_state: SeatState<Self>,
//! # };
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # impl SeatHandler for State {
//! #     type KeyboardFocus = Target;
//! #     type PointerFocus = Target;
//! #     type TouchFocus = Target;
//! #
//! #     fn seat_state(&mut self) -> &mut SeatState<Self> {
//! #         &mut self.seat_state
//! #     }
//! # }
//! let state = RelativePointerManagerState::new::<State>(&display.handle());
//! ```

use wayland_protocols::wp::relative_pointer::zv1::server::{
    zwp_relative_pointer_manager_v1::{self, ZwpRelativePointerManagerV1},
    zwp_relative_pointer_v1::{self, ZwpRelativePointerV1},
};
use wayland_server::{
    backend::{ClientId, GlobalId},
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::{
    input::{pointer::PointerHandle, SeatHandler, SeatState},
    wayland::seat::PointerUserData,
};

const MANAGER_VERSION: u32 = 1;

/// User data of ZwpRelativePointerV1 object
#[derive(Debug)]
pub struct RelativePointerUserData<D: SeatHandler> {
    handle: Option<PointerHandle<D>>,
}

/// State of the relative pointer manager
#[derive(Debug)]
pub struct RelativePointerManagerState {
    global: GlobalId,
}

impl RelativePointerManagerState {
    /// Register new [ZwpRelativePointerV1] global
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: SeatHandler,
    {
        let global =
            display.create_delegated_global::<D, ZwpRelativePointerManagerV1, _, Self>(MANAGER_VERSION, ());

        Self { global }
    }

    /// [ZwpRelativePointerV1] GlobalId getter
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> Dispatch<ZwpRelativePointerManagerV1, (), D> for RelativePointerManagerState
where
    D: SeatHandler,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _relative_pointer_manager: &ZwpRelativePointerManagerV1,
        request: zwp_relative_pointer_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_relative_pointer_manager_v1::Request::GetRelativePointer { id, pointer } => {
                let handle = &pointer
                    .delegated_data::<PointerUserData<D>, SeatState<D>>()
                    .unwrap()
                    .handle;
                let user_data = RelativePointerUserData {
                    handle: handle.clone(),
                };
                let pointer = data_init.init_delegated::<_, _, Self>(id, user_data);
                if let Some(handle) = handle {
                    handle.new_relative_pointer(pointer);
                }
            }
            zwp_relative_pointer_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D> GlobalDispatch<ZwpRelativePointerManagerV1, (), D> for RelativePointerManagerState
where
    D: SeatHandler,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwpRelativePointerManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init_delegated::<_, _, Self>(resource, ());
    }
}

impl<D> Dispatch<ZwpRelativePointerV1, RelativePointerUserData<D>, D> for RelativePointerManagerState
where
    D: SeatHandler,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _relative_pointer: &ZwpRelativePointerV1,
        request: zwp_relative_pointer_v1::Request,
        _data: &RelativePointerUserData<D>,
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_relative_pointer_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _: ClientId,
        object: &ZwpRelativePointerV1,
        data: &RelativePointerUserData<D>,
    ) {
        if let Some(ref handle) = data.handle {
            handle
                .known_relative_pointers
                .lock()
                .unwrap()
                .retain(|p| p.id() != object.id());
        }
    }
}

/// Macro to delegate implementation of the relative pointer protocol
#[deprecated(note = "No longer needed, this is now NOP")]
#[macro_export]
macro_rules! delegate_relative_pointer {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {};
}
