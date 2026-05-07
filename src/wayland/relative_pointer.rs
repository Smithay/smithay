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
//!
//! smithay::delegate_dispatch2!(State);
//! ```

use std::sync::{Arc, Mutex, atomic::Ordering};

use atomic_float::AtomicF64;
use wayland_protocols::wp::relative_pointer::zv1::server::{
    zwp_relative_pointer_manager_v1::{self, ZwpRelativePointerManagerV1},
    zwp_relative_pointer_v1::{self, ZwpRelativePointerV1},
};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
    backend::{ClientId, GlobalId},
    protocol::wl_surface::WlSurface,
};

use crate::{
    input::{
        SeatHandler,
        pointer::{PointerHandle, RelativeMotionEvent},
    },
    wayland::{Dispatch2, GlobalData, GlobalDispatch2, seat::PointerUserData},
};

const MANAGER_VERSION: u32 = 1;

#[derive(Debug, Default)]
pub(crate) struct WpRelativePointerHandle {
    known_relative_pointers: Mutex<Vec<ZwpRelativePointerV1>>,
}

impl WpRelativePointerHandle {
    fn new_relative_pointer(&self, pointer: ZwpRelativePointerV1) {
        self.known_relative_pointers.lock().unwrap().push(pointer);
    }

    pub(super) fn relative_motion<D: SeatHandler + 'static>(
        &self,
        surface: &WlSurface,
        event: &RelativeMotionEvent,
    ) {
        self.for_each_focused_pointer(surface, |ptr| {
            let client_scale = ptr
                .data::<RelativePointerUserData<D>>()
                .unwrap()
                .client_scale
                .load(Ordering::Acquire);
            let delta = event.delta.to_client(client_scale);
            let delta_unaccel = event.delta_unaccel;

            let utime_hi = (event.utime >> 32) as u32;
            let utime_lo = (event.utime & 0xffffffff) as u32;
            ptr.relative_motion(
                utime_hi,
                utime_lo,
                delta.x,
                delta.y,
                delta_unaccel.x,
                delta_unaccel.y,
            );
        })
    }

    fn for_each_focused_pointer(&self, surface: &WlSurface, mut f: impl FnMut(ZwpRelativePointerV1)) {
        let inner = self.known_relative_pointers.lock().unwrap();
        for ptr in &*inner {
            if ptr.id().same_client_as(&surface.id()) {
                f(ptr.clone())
            }
        }
    }
}

/// User data of ZwpRelativePointerV1 object
#[derive(Debug)]
pub struct RelativePointerUserData<D: SeatHandler> {
    handle: Option<PointerHandle<D>>,
    client_scale: Arc<AtomicF64>,
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
        D: GlobalDispatch<ZwpRelativePointerManagerV1, GlobalData>,
        D: Dispatch<ZwpRelativePointerManagerV1, GlobalData>,
        D: Dispatch<ZwpRelativePointerV1, RelativePointerUserData<D>>,
        D: SeatHandler,
        D: 'static,
    {
        let global = display.create_global::<D, ZwpRelativePointerManagerV1, _>(MANAGER_VERSION, GlobalData);

        Self { global }
    }

    /// [ZwpRelativePointerV1] GlobalId getter
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> Dispatch2<ZwpRelativePointerManagerV1, D> for GlobalData
where
    D: Dispatch<ZwpRelativePointerV1, RelativePointerUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &wayland_server::Client,
        _relative_pointer_manager: &ZwpRelativePointerManagerV1,
        request: zwp_relative_pointer_manager_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_relative_pointer_manager_v1::Request::GetRelativePointer { id, pointer } => {
                let data = pointer.data::<PointerUserData<D>>().unwrap();
                let user_data = RelativePointerUserData {
                    handle: data.handle.clone(),
                    client_scale: data.client_scale.clone(),
                };
                let pointer = data_init.init(id, user_data);
                if let Some(handle) = &data.handle {
                    handle.wp_relative.new_relative_pointer(pointer);
                }
            }
            zwp_relative_pointer_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D> GlobalDispatch2<ZwpRelativePointerManagerV1, D> for GlobalData
where
    D: Dispatch<ZwpRelativePointerManagerV1, GlobalData> + SeatHandler + 'static,
{
    fn bind(
        &self,
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwpRelativePointerManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }
}

impl<D> Dispatch2<ZwpRelativePointerV1, D> for RelativePointerUserData<D>
where
    D: SeatHandler,
    D: 'static,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &wayland_server::Client,
        _relative_pointer: &ZwpRelativePointerV1,
        request: zwp_relative_pointer_v1::Request,
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_relative_pointer_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(&self, _state: &mut D, _: ClientId, object: &ZwpRelativePointerV1) {
        if let Some(ref handle) = self.handle {
            handle
                .wp_relative
                .known_relative_pointers
                .lock()
                .unwrap()
                .retain(|p| p.id() != object.id());
        }
    }
}
