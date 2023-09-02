//! Utilities for pointer gestures support
//!
//! This protocol allows clients to receive touchpad gestures. Use the following methods to send
//! gesture events to any respective objects created by the client:
//!
//! 1. Swipe gestures
//!     - [`PointerHandle::gesture_swipe_begin`]
//!     - [`PointerHandle::gesture_swipe_update`]
//!     - [`PointerHandle::gesture_swipe_end`]
//! 2. Pinch gesture:
//!     - [`PointerHandle::gesture_pinch_begin`]
//!     - [`PointerHandle::gesture_pinch_update`]
//!     - [`PointerHandle::gesture_pinch_end`]
//! 3. Hold gesture:
//!     - [`PointerHandle::gesture_hold_begin`]
//!     - [`PointerHandle::gesture_hold_end`]
//!
//! ```
//! extern crate wayland_server;
//! extern crate smithay;
//!
//! use smithay::wayland::pointer_gestures::PointerGesturesState;
//! use smithay::delegate_pointer_gestures;
//! # use smithay::backend::input::KeyState;
//! # use smithay::input::{
//! #   pointer::{PointerTarget, AxisFrame, MotionEvent, ButtonEvent, RelativeMotionEvent,
//! #             GestureSwipeBeginEvent, GestureSwipeUpdateEvent, GestureSwipeEndEvent,
//! #             GesturePinchBeginEvent, GesturePinchUpdateEvent, GesturePinchEndEvent,
//! #             GestureHoldBeginEvent, GestureHoldEndEvent},
//! #   keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
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
//! # struct State {
//! #     seat_state: SeatState<Self>,
//! # };
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # impl SeatHandler for State {
//! #     type KeyboardFocus = Target;
//! #     type PointerFocus = Target;
//! #
//! #     fn seat_state(&mut self) -> &mut SeatState<Self> {
//! #         &mut self.seat_state
//! #     }
//! # }
//! let state = PointerGesturesState::new::<State>(&display.handle());
//!
//! delegate_pointer_gestures!(State);
//! ```

use std::sync::Mutex;

use wayland_protocols::wp::pointer_gestures::zv1::server::{
    zwp_pointer_gesture_hold_v1::{self, ZwpPointerGestureHoldV1},
    zwp_pointer_gesture_pinch_v1::{self, ZwpPointerGesturePinchV1},
    zwp_pointer_gesture_swipe_v1::{self, ZwpPointerGestureSwipeV1},
    zwp_pointer_gestures_v1::{self, ZwpPointerGesturesV1},
};
use wayland_server::{
    backend::{ClientId, GlobalId},
    protocol::wl_surface::WlSurface,
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::{
    input::{pointer::PointerHandle, SeatHandler},
    wayland::seat::PointerUserData,
};

const MANAGER_VERSION: u32 = 3;

/// User data of ZwpPointerGesture*V1 objects
#[derive(Debug)]
pub struct PointerGestureUserData<D: SeatHandler> {
    handle: Option<PointerHandle<D>>,
    /// This gesture is in the middle between its begin() and end() on this surface.
    pub(crate) in_progress_on: Mutex<Option<WlSurface>>,
}

/// State of the pointer gestures
#[derive(Debug)]
pub struct PointerGesturesState {
    global: GlobalId,
}

impl PointerGesturesState {
    /// Register new [ZwpPointerGesturesV1] global
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwpPointerGesturesV1, ()>,
        D: Dispatch<ZwpPointerGesturesV1, ()>,
        D: Dispatch<ZwpPointerGestureSwipeV1, PointerGestureUserData<D>>,
        D: Dispatch<ZwpPointerGesturePinchV1, PointerGestureUserData<D>>,
        D: Dispatch<ZwpPointerGestureHoldV1, PointerGestureUserData<D>>,
        D: SeatHandler,
        D: 'static,
    {
        let global = display.create_global::<D, ZwpPointerGesturesV1, _>(MANAGER_VERSION, ());

        Self { global }
    }

    /// [ZwpPointerGesturesV1] GlobalId getter
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> Dispatch<ZwpPointerGesturesV1, (), D> for PointerGesturesState
where
    D: Dispatch<ZwpPointerGesturesV1, ()>,
    D: Dispatch<ZwpPointerGestureSwipeV1, PointerGestureUserData<D>>,
    D: Dispatch<ZwpPointerGesturePinchV1, PointerGestureUserData<D>>,
    D: Dispatch<ZwpPointerGestureHoldV1, PointerGestureUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _pointer_gestures: &ZwpPointerGesturesV1,
        request: zwp_pointer_gestures_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_pointer_gestures_v1::Request::GetSwipeGesture { id, pointer } => {
                let handle = &pointer.data::<PointerUserData<D>>().unwrap().handle;
                let user_data = PointerGestureUserData {
                    handle: handle.clone(),
                    in_progress_on: Mutex::new(None),
                };
                let gesture = data_init.init(id, user_data);
                if let Some(handle) = handle {
                    handle.new_swipe_gesture(gesture);
                }
            }
            zwp_pointer_gestures_v1::Request::GetPinchGesture { id, pointer } => {
                let handle = &pointer.data::<PointerUserData<D>>().unwrap().handle;
                let user_data = PointerGestureUserData {
                    handle: handle.clone(),
                    in_progress_on: Mutex::new(None),
                };
                let gesture = data_init.init(id, user_data);
                if let Some(handle) = handle {
                    handle.new_pinch_gesture(gesture);
                }
            }
            zwp_pointer_gestures_v1::Request::GetHoldGesture { id, pointer } => {
                let handle = &pointer.data::<PointerUserData<D>>().unwrap().handle;
                let user_data = PointerGestureUserData {
                    handle: handle.clone(),
                    in_progress_on: Mutex::new(None),
                };
                let gesture = data_init.init(id, user_data);
                if let Some(handle) = handle {
                    handle.new_hold_gesture(gesture);
                }
            }
            zwp_pointer_gestures_v1::Request::Release => {}
            _ => unreachable!(),
        }
    }
}

impl<D> GlobalDispatch<ZwpPointerGesturesV1, (), D> for PointerGesturesState
where
    D: GlobalDispatch<ZwpPointerGesturesV1, ()> + Dispatch<ZwpPointerGesturesV1, ()> + SeatHandler + 'static,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwpPointerGesturesV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<ZwpPointerGestureSwipeV1, PointerGestureUserData<D>, D> for PointerGesturesState
where
    D: Dispatch<ZwpPointerGestureSwipeV1, PointerGestureUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _gesture: &ZwpPointerGestureSwipeV1,
        request: zwp_pointer_gesture_swipe_v1::Request,
        _data: &PointerGestureUserData<D>,
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_pointer_gesture_swipe_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _: ClientId,
        object: &ZwpPointerGestureSwipeV1,
        data: &PointerGestureUserData<D>,
    ) {
        if let Some(ref handle) = data.handle {
            handle
                .known_swipe_gestures
                .lock()
                .unwrap()
                .retain(|p| p.id() != object.id());
        }
    }
}

impl<D> Dispatch<ZwpPointerGesturePinchV1, PointerGestureUserData<D>, D> for PointerGesturesState
where
    D: Dispatch<ZwpPointerGesturePinchV1, PointerGestureUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _gesture: &ZwpPointerGesturePinchV1,
        request: zwp_pointer_gesture_pinch_v1::Request,
        _data: &PointerGestureUserData<D>,
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_pointer_gesture_pinch_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _: ClientId,
        object: &ZwpPointerGesturePinchV1,
        data: &PointerGestureUserData<D>,
    ) {
        if let Some(ref handle) = data.handle {
            handle
                .known_pinch_gestures
                .lock()
                .unwrap()
                .retain(|p| p.id() != object.id());
        }
    }
}

impl<D> Dispatch<ZwpPointerGestureHoldV1, PointerGestureUserData<D>, D> for PointerGesturesState
where
    D: Dispatch<ZwpPointerGestureHoldV1, PointerGestureUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _gesture: &ZwpPointerGestureHoldV1,
        request: zwp_pointer_gesture_hold_v1::Request,
        _data: &PointerGestureUserData<D>,
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_pointer_gesture_hold_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _: ClientId,
        object: &ZwpPointerGestureHoldV1,
        data: &PointerGestureUserData<D>,
    ) {
        if let Some(ref handle) = data.handle {
            handle
                .known_hold_gestures
                .lock()
                .unwrap()
                .retain(|p| p.id() != object.id());
        }
    }
}

/// Macro to delegate implementation of the pointer gestures protocol
#[macro_export]
macro_rules! delegate_pointer_gestures {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::pointer_gestures::zv1::server::zwp_pointer_gestures_v1::ZwpPointerGesturesV1: ()
        ] => $crate::wayland::pointer_gestures::PointerGesturesState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::pointer_gestures::zv1::server::zwp_pointer_gestures_v1::ZwpPointerGesturesV1: ()
        ] => $crate::wayland::pointer_gestures::PointerGesturesState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::pointer_gestures::zv1::server::zwp_pointer_gesture_swipe_v1::ZwpPointerGestureSwipeV1: $crate::wayland::pointer_gestures::PointerGestureUserData<Self>
        ] => $crate::wayland::pointer_gestures::PointerGesturesState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::pointer_gestures::zv1::server::zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1: $crate::wayland::pointer_gestures::PointerGestureUserData<Self>
        ] => $crate::wayland::pointer_gestures::PointerGesturesState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::pointer_gestures::zv1::server::zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1: $crate::wayland::pointer_gestures::PointerGestureUserData<Self>
        ] => $crate::wayland::pointer_gestures::PointerGesturesState);
    };
}
