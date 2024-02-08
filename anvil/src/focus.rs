use smithay::input::pointer::{
    GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
    GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
};
pub use smithay::{
    backend::input::KeyState,
    desktop::{LayerSurface, PopupKind},
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerTarget, RelativeMotionEvent},
        Seat,
    },
    reexports::wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface, Resource},
    utils::{IsAlive, Serial},
    wayland::seat::WaylandFocus,
};

use crate::{
    shell::WindowElement,
    state::{AnvilState, Backend},
};

#[derive(Debug, Clone, PartialEq)]
pub enum FocusTarget {
    Window(WindowElement),
    LayerSurface(LayerSurface),
    Popup(PopupKind),
}

impl IsAlive for FocusTarget {
    fn alive(&self) -> bool {
        match self {
            FocusTarget::Window(w) => w.alive(),
            FocusTarget::LayerSurface(l) => l.alive(),
            FocusTarget::Popup(p) => p.alive(),
        }
    }
}

impl From<FocusTarget> for WlSurface {
    fn from(target: FocusTarget) -> Self {
        target.wl_surface().unwrap()
    }
}

impl<BackendData: Backend> PointerTarget<AnvilState<BackendData>> for FocusTarget {
    fn enter(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &MotionEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::enter(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::enter(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::enter(p.wl_surface(), seat, data, event),
        }
    }
    fn motion(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &MotionEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::motion(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::motion(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::motion(p.wl_surface(), seat, data, event),
        }
    }
    fn relative_motion(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &RelativeMotionEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::relative_motion(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::relative_motion(l.wl_surface(), seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::relative_motion(p.wl_surface(), seat, data, event),
        }
    }
    fn button(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &ButtonEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::button(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::button(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::button(p.wl_surface(), seat, data, event),
        }
    }
    fn axis(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        frame: AxisFrame,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::axis(w, seat, data, frame),
            FocusTarget::LayerSurface(l) => PointerTarget::axis(l, seat, data, frame),
            FocusTarget::Popup(p) => PointerTarget::axis(p.wl_surface(), seat, data, frame),
        }
    }
    fn frame(&self, seat: &Seat<AnvilState<BackendData>>, data: &mut AnvilState<BackendData>) {
        match self {
            FocusTarget::Window(w) => PointerTarget::frame(w, seat, data),
            FocusTarget::LayerSurface(l) => PointerTarget::frame(l, seat, data),
            FocusTarget::Popup(p) => PointerTarget::frame(p.wl_surface(), seat, data),
        }
    }
    fn leave(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        serial: Serial,
        time: u32,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::leave(w, seat, data, serial, time),
            FocusTarget::LayerSurface(l) => PointerTarget::leave(l, seat, data, serial, time),
            FocusTarget::Popup(p) => PointerTarget::leave(p.wl_surface(), seat, data, serial, time),
        }
    }
    fn gesture_swipe_begin(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GestureSwipeBeginEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::gesture_swipe_begin(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::gesture_swipe_begin(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::gesture_swipe_begin(p.wl_surface(), seat, data, event),
        }
    }
    fn gesture_swipe_update(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GestureSwipeUpdateEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::gesture_swipe_update(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::gesture_swipe_update(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::gesture_swipe_update(p.wl_surface(), seat, data, event),
        }
    }
    fn gesture_swipe_end(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GestureSwipeEndEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::gesture_swipe_end(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::gesture_swipe_end(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::gesture_swipe_end(p.wl_surface(), seat, data, event),
        }
    }
    fn gesture_pinch_begin(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GesturePinchBeginEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::gesture_pinch_begin(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::gesture_pinch_begin(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::gesture_pinch_begin(p.wl_surface(), seat, data, event),
        }
    }
    fn gesture_pinch_update(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GesturePinchUpdateEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::gesture_pinch_update(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::gesture_pinch_update(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::gesture_pinch_update(p.wl_surface(), seat, data, event),
        }
    }
    fn gesture_pinch_end(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GesturePinchEndEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::gesture_pinch_end(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::gesture_pinch_end(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::gesture_pinch_end(p.wl_surface(), seat, data, event),
        }
    }
    fn gesture_hold_begin(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GestureHoldBeginEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::gesture_hold_begin(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::gesture_hold_begin(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::gesture_hold_begin(p.wl_surface(), seat, data, event),
        }
    }
    fn gesture_hold_end(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GestureHoldEndEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::gesture_hold_end(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::gesture_hold_end(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::gesture_hold_end(p.wl_surface(), seat, data, event),
        }
    }
}

impl<BackendData: Backend> KeyboardTarget<AnvilState<BackendData>> for FocusTarget {
    fn enter(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            FocusTarget::Window(w) => KeyboardTarget::enter(w, seat, data, keys, serial),
            FocusTarget::LayerSurface(l) => KeyboardTarget::enter(l, seat, data, keys, serial),
            FocusTarget::Popup(p) => KeyboardTarget::enter(p.wl_surface(), seat, data, keys, serial),
        }
    }
    fn leave(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        serial: Serial,
    ) {
        match self {
            FocusTarget::Window(w) => KeyboardTarget::leave(w, seat, data, serial),
            FocusTarget::LayerSurface(l) => KeyboardTarget::leave(l, seat, data, serial),
            FocusTarget::Popup(p) => KeyboardTarget::leave(p.wl_surface(), seat, data, serial),
        }
    }
    fn key(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match self {
            FocusTarget::Window(w) => KeyboardTarget::key(w, seat, data, key, state, serial, time),
            FocusTarget::LayerSurface(l) => KeyboardTarget::key(l, seat, data, key, state, serial, time),
            FocusTarget::Popup(p) => {
                KeyboardTarget::key(p.wl_surface(), seat, data, key, state, serial, time)
            }
        }
    }
    fn modifiers(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self {
            FocusTarget::Window(w) => KeyboardTarget::modifiers(w, seat, data, modifiers, serial),
            FocusTarget::LayerSurface(l) => KeyboardTarget::modifiers(l, seat, data, modifiers, serial),
            FocusTarget::Popup(p) => KeyboardTarget::modifiers(p.wl_surface(), seat, data, modifiers, serial),
        }
    }
}

impl WaylandFocus for FocusTarget {
    fn wl_surface(&self) -> Option<WlSurface> {
        match self {
            FocusTarget::Window(w) => w.wl_surface(),
            FocusTarget::LayerSurface(l) => Some(l.wl_surface().clone()),
            FocusTarget::Popup(p) => Some(p.wl_surface().clone()),
        }
    }
    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            FocusTarget::Window(w) => w.0.same_client_as(object_id),
            FocusTarget::LayerSurface(l) => l.wl_surface().id().same_client_as(object_id),
            FocusTarget::Popup(p) => p.wl_surface().id().same_client_as(object_id),
        }
    }
}

impl From<WindowElement> for FocusTarget {
    fn from(w: WindowElement) -> Self {
        FocusTarget::Window(w)
    }
}

impl From<LayerSurface> for FocusTarget {
    fn from(l: LayerSurface) -> Self {
        FocusTarget::LayerSurface(l)
    }
}

impl From<PopupKind> for FocusTarget {
    fn from(p: PopupKind) -> Self {
        FocusTarget::Popup(p)
    }
}
