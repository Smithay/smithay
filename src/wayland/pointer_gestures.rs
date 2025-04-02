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
//! let state = PointerGesturesState::new::<State>(&display.handle());
//!
//! delegate_pointer_gestures!(State);
//! ```

use std::sync::{atomic::Ordering, Arc, Mutex};

use atomic_float::AtomicF64;
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
    input::{
        pointer::{
            GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
            PointerHandle,
        },
        SeatHandler,
    },
    utils::{Serial, SERIAL_COUNTER},
    wayland::seat::PointerUserData,
};

const MANAGER_VERSION: u32 = 3;

#[derive(Debug, Default)]
pub(crate) struct WpPointerGesturePointerHandle {
    known_swipe_gestures: Mutex<Vec<ZwpPointerGestureSwipeV1>>,
    known_pinch_gestures: Mutex<Vec<ZwpPointerGesturePinchV1>>,
    known_hold_gestures: Mutex<Vec<ZwpPointerGestureHoldV1>>,
}

impl WpPointerGesturePointerHandle {
    fn new_swipe_gesture(&self, gesture: ZwpPointerGestureSwipeV1) {
        self.known_swipe_gestures.lock().unwrap().push(gesture);
    }

    fn new_pinch_gesture(&self, gesture: ZwpPointerGesturePinchV1) {
        self.known_pinch_gestures.lock().unwrap().push(gesture);
    }

    fn new_hold_gesture(&self, gesture: ZwpPointerGestureHoldV1) {
        self.known_hold_gestures.lock().unwrap().push(gesture);
    }

    pub(super) fn leave<D: SeatHandler + 'static>(&self, surface: &WlSurface, serial: Serial, time: u32) {
        self.for_each_focused_swipe_gesture(surface, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().take();
            if ongoing.is_some() {
                // Cancel the ongoing gesture.
                gesture.end(serial.into(), time, 1);
            }
        });
        self.for_each_focused_pinch_gesture(surface, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().take();
            if ongoing.is_some() {
                // Cancel the ongoing gesture.
                gesture.end(serial.into(), time, 1);
            }
        });
        self.for_each_focused_hold_gesture(surface, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().take();
            if ongoing.is_some() {
                // Cancel the ongoing gesture.
                gesture.end(serial.into(), time, 1);
            }
        });
    }

    pub(super) fn gesture_swipe_begin<D: SeatHandler + 'static>(
        &self,
        surface: &WlSurface,
        event: &GestureSwipeBeginEvent,
    ) {
        self.for_each_focused_swipe_gesture(surface, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().replace(surface.clone());
            if ongoing.is_some() {
                // Cancel an ongoing gesture for a different surface.
                gesture.end(event.serial.into(), event.time, 1);
            }
            gesture.begin(event.serial.into(), event.time, surface, event.fingers);
        });
    }

    pub(super) fn gesture_swipe_update<D: SeatHandler + 'static>(
        &self,
        surface: &WlSurface,
        event: &GestureSwipeUpdateEvent,
    ) {
        self.for_each_focused_swipe_gesture(surface, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let mut ongoing = data.in_progress_on.lock().unwrap();
            // Check that the ongoing gesture is for this surface.
            if ongoing.as_ref() == Some(surface) {
                let client_scale = data.client_scale.load(Ordering::Acquire);
                let delta = event.delta.to_client(client_scale);
                gesture.update(event.time, delta.x, delta.y);
            } else if ongoing.take().is_some() {
                // If it was for a different surface, cancel it.
                gesture.end(SERIAL_COUNTER.next_serial().into(), event.time, 1);
            }
        });
    }

    pub(super) fn gesture_swipe_end<D: SeatHandler + 'static>(
        &self,
        surface: &WlSurface,
        event: &GestureSwipeEndEvent,
    ) {
        self.for_each_focused_swipe_gesture(surface, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().take();
            // Check if the gesture was ongoing.
            if ongoing.is_some() {
                let cancelled = if ongoing.as_ref() == Some(surface) {
                    event.cancelled
                } else {
                    // If the gesture was ongoing for any other surface then cancel it.
                    true
                };
                gesture.end(event.serial.into(), event.time, cancelled.into());
            }
        });
    }

    pub(super) fn gesture_pinch_begin<D: SeatHandler + 'static>(
        &self,
        surface: &WlSurface,
        event: &GesturePinchBeginEvent,
    ) {
        self.for_each_focused_pinch_gesture(surface, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().replace(surface.clone());
            if ongoing.is_some() {
                // Cancel an ongoing gesture for a different surface.
                gesture.end(event.serial.into(), event.time, 1);
            }
            gesture.begin(event.serial.into(), event.time, surface, event.fingers);
        });
    }

    pub(super) fn gesture_pinch_update<D: SeatHandler + 'static>(
        &self,
        surface: &WlSurface,
        event: &GesturePinchUpdateEvent,
    ) {
        self.for_each_focused_pinch_gesture(surface, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let mut ongoing = data.in_progress_on.lock().unwrap();
            // Check that the ongoing gesture is for this surface.
            if ongoing.as_ref() == Some(surface) {
                let client_scale = data.client_scale.load(Ordering::Acquire);
                let delta = event.delta.to_client(client_scale);
                gesture.update(event.time, delta.x, delta.y, event.scale, event.rotation);
            } else if ongoing.take().is_some() {
                // If it was for a different surface, cancel it.
                gesture.end(SERIAL_COUNTER.next_serial().into(), event.time, 1);
            }
        });
    }

    pub(super) fn gesture_pinch_end<D: SeatHandler + 'static>(
        &self,
        surface: &WlSurface,
        event: &GesturePinchEndEvent,
    ) {
        self.for_each_focused_pinch_gesture(surface, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().take();
            // Check if the gesture was ongoing.
            if ongoing.is_some() {
                let cancelled = if ongoing.as_ref() == Some(surface) {
                    event.cancelled
                } else {
                    // If the gesture was ongoing for any other surface then cancel it.
                    true
                };
                gesture.end(event.serial.into(), event.time, cancelled.into());
            }
        });
    }

    pub(super) fn gesture_hold_begin<D: SeatHandler + 'static>(
        &self,
        surface: &WlSurface,
        event: &GestureHoldBeginEvent,
    ) {
        self.for_each_focused_hold_gesture(surface, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().replace(surface.clone());
            if ongoing.is_some() {
                // Cancel an ongoing gesture for a different surface.
                gesture.end(event.serial.into(), event.time, 1);
            }
            gesture.begin(event.serial.into(), event.time, surface, event.fingers);
        });
    }

    pub(super) fn gesture_hold_end<D: SeatHandler + 'static>(
        &self,
        surface: &WlSurface,
        event: &GestureHoldEndEvent,
    ) {
        self.for_each_focused_hold_gesture(surface, |gesture| {
            let data = gesture.data::<PointerGestureUserData<D>>().unwrap();
            let ongoing = data.in_progress_on.lock().unwrap().take();
            // Check if the gesture was ongoing.
            if ongoing.is_some() {
                let cancelled = if ongoing.as_ref() == Some(surface) {
                    event.cancelled
                } else {
                    // If the gesture was ongoing for any other surface then cancel it.
                    true
                };
                gesture.end(event.serial.into(), event.time, cancelled.into());
            }
        });
    }

    fn for_each_focused_swipe_gesture(
        &self,
        surface: &WlSurface,
        mut f: impl FnMut(ZwpPointerGestureSwipeV1),
    ) {
        let inner = self.known_swipe_gestures.lock().unwrap();
        for ptr in &*inner {
            if ptr.id().same_client_as(&surface.id()) {
                f(ptr.clone())
            }
        }
    }

    fn for_each_focused_pinch_gesture(
        &self,
        surface: &WlSurface,
        mut f: impl FnMut(ZwpPointerGesturePinchV1),
    ) {
        let inner = self.known_pinch_gestures.lock().unwrap();
        for ptr in &*inner {
            if ptr.id().same_client_as(&surface.id()) {
                f(ptr.clone())
            }
        }
    }

    fn for_each_focused_hold_gesture(&self, surface: &WlSurface, mut f: impl FnMut(ZwpPointerGestureHoldV1)) {
        let inner = self.known_hold_gestures.lock().unwrap();
        for ptr in &*inner {
            if ptr.id().same_client_as(&surface.id()) {
                f(ptr.clone())
            }
        }
    }
}

/// User data of ZwpPointerGesture*V1 objects
#[derive(Debug)]
pub struct PointerGestureUserData<D: SeatHandler> {
    handle: Option<PointerHandle<D>>,
    /// This gesture is in the middle between its begin() and end() on this surface.
    pub(crate) in_progress_on: Mutex<Option<WlSurface>>,
    client_scale: Arc<AtomicF64>,
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
                let data = pointer.data::<PointerUserData<D>>().unwrap();
                let user_data = PointerGestureUserData {
                    handle: data.handle.clone(),
                    in_progress_on: Mutex::new(None),
                    client_scale: data.client_scale.clone(),
                };
                let gesture = data_init.init(id, user_data);
                if let Some(handle) = &data.handle {
                    handle.wp_pointer_gestures.new_swipe_gesture(gesture);
                }
            }
            zwp_pointer_gestures_v1::Request::GetPinchGesture { id, pointer } => {
                let data = pointer.data::<PointerUserData<D>>().unwrap();
                let user_data = PointerGestureUserData {
                    handle: data.handle.clone(),
                    in_progress_on: Mutex::new(None),
                    client_scale: data.client_scale.clone(),
                };
                let gesture = data_init.init(id, user_data);
                if let Some(handle) = &data.handle {
                    handle.wp_pointer_gestures.new_pinch_gesture(gesture);
                }
            }
            zwp_pointer_gestures_v1::Request::GetHoldGesture { id, pointer } => {
                let data = pointer.data::<PointerUserData<D>>().unwrap();
                let user_data = PointerGestureUserData {
                    handle: data.handle.clone(),
                    in_progress_on: Mutex::new(None),
                    client_scale: data.client_scale.clone(),
                };
                let gesture = data_init.init(id, user_data);
                if let Some(handle) = &data.handle {
                    handle.wp_pointer_gestures.new_hold_gesture(gesture);
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
                .wp_pointer_gestures
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
                .wp_pointer_gestures
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
                .wp_pointer_gestures
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
