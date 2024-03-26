#[cfg(feature = "xwayland")]
use smithay::xwayland::X11Surface;
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
use smithay::{
    desktop::{Window, WindowSurface},
    input::{
        pointer::{
            GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
        },
        touch::TouchTarget,
    },
};

use crate::{
    shell::{WindowElement, SSD},
    state::{AnvilState, Backend},
};

#[derive(Debug, Clone, PartialEq)]
pub enum KeyboardFocusTarget {
    Window(Window),
    LayerSurface(LayerSurface),
    Popup(PopupKind),
}

impl IsAlive for KeyboardFocusTarget {
    fn alive(&self) -> bool {
        match self {
            KeyboardFocusTarget::Window(w) => w.alive(),
            KeyboardFocusTarget::LayerSurface(l) => l.alive(),
            KeyboardFocusTarget::Popup(p) => p.alive(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PointerFocusTarget {
    WlSurface(WlSurface),
    #[cfg(feature = "xwayland")]
    X11Surface(X11Surface),
    SSD(SSD),
}

impl IsAlive for PointerFocusTarget {
    fn alive(&self) -> bool {
        match self {
            PointerFocusTarget::WlSurface(w) => w.alive(),
            #[cfg(feature = "xwayland")]
            PointerFocusTarget::X11Surface(w) => w.alive(),
            PointerFocusTarget::SSD(x) => x.alive(),
        }
    }
}

impl From<PointerFocusTarget> for WlSurface {
    fn from(target: PointerFocusTarget) -> Self {
        target.wl_surface().unwrap()
    }
}

impl KeyboardFocusTarget {
    fn inner_keyboard_target<BackendData: Backend>(&self) -> &dyn KeyboardTarget<AnvilState<BackendData>> {
        match self {
            Self::Window(w) => match w.underlying_surface() {
                WindowSurface::Wayland(w) => w.wl_surface(),
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(s) => s,
            },
            Self::LayerSurface(l) => l.wl_surface(),
            Self::Popup(p) => p.wl_surface(),
        }
    }
}

impl PointerFocusTarget {
    fn inner_pointer_target<BackendData: Backend>(&self) -> &dyn PointerTarget<AnvilState<BackendData>> {
        match self {
            Self::WlSurface(w) => w,
            #[cfg(feature = "xwayland")]
            Self::X11Surface(w) => w,
            Self::SSD(w) => w,
        }
    }

    fn inner_touch_target<BackendData: Backend>(&self) -> &dyn TouchTarget<AnvilState<BackendData>> {
        match self {
            Self::WlSurface(w) => w,
            #[cfg(feature = "xwayland")]
            Self::X11Surface(w) => w,
            Self::SSD(w) => w,
        }
    }
}

impl<BackendData: Backend> PointerTarget<AnvilState<BackendData>> for PointerFocusTarget {
    fn enter(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &MotionEvent,
    ) {
        self.inner_pointer_target().enter(seat, data, event)
    }
    fn motion(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &MotionEvent,
    ) {
        self.inner_pointer_target().motion(seat, data, event)
    }
    fn relative_motion(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &RelativeMotionEvent,
    ) {
        self.inner_pointer_target().relative_motion(seat, data, event)
    }
    fn button(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &ButtonEvent,
    ) {
        self.inner_pointer_target().button(seat, data, event)
    }
    fn axis(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        frame: AxisFrame,
    ) {
        self.inner_pointer_target().axis(seat, data, frame)
    }
    fn frame(&self, seat: &Seat<AnvilState<BackendData>>, data: &mut AnvilState<BackendData>) {
        self.inner_pointer_target().frame(seat, data)
    }
    fn leave(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        serial: Serial,
        time: u32,
    ) {
        self.inner_pointer_target().leave(seat, data, serial, time)
    }
    fn gesture_swipe_begin(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GestureSwipeBeginEvent,
    ) {
        self.inner_pointer_target().gesture_swipe_begin(seat, data, event)
    }
    fn gesture_swipe_update(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GestureSwipeUpdateEvent,
    ) {
        self.inner_pointer_target()
            .gesture_swipe_update(seat, data, event)
    }
    fn gesture_swipe_end(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GestureSwipeEndEvent,
    ) {
        self.inner_pointer_target().gesture_swipe_end(seat, data, event)
    }
    fn gesture_pinch_begin(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GesturePinchBeginEvent,
    ) {
        self.inner_pointer_target().gesture_pinch_begin(seat, data, event)
    }
    fn gesture_pinch_update(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GesturePinchUpdateEvent,
    ) {
        self.inner_pointer_target()
            .gesture_pinch_update(seat, data, event)
    }
    fn gesture_pinch_end(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GesturePinchEndEvent,
    ) {
        self.inner_pointer_target().gesture_pinch_end(seat, data, event)
    }
    fn gesture_hold_begin(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GestureHoldBeginEvent,
    ) {
        self.inner_pointer_target().gesture_hold_begin(seat, data, event)
    }
    fn gesture_hold_end(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &GestureHoldEndEvent,
    ) {
        self.inner_pointer_target().gesture_hold_end(seat, data, event)
    }
}

impl<BackendData: Backend> KeyboardTarget<AnvilState<BackendData>> for KeyboardFocusTarget {
    fn enter(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        self.inner_keyboard_target().enter(seat, data, keys, serial)
    }
    fn leave(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        serial: Serial,
    ) {
        self.inner_keyboard_target().leave(seat, data, serial)
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
        self.inner_keyboard_target()
            .key(seat, data, key, state, serial, time)
    }
    fn modifiers(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        self.inner_keyboard_target()
            .modifiers(seat, data, modifiers, serial)
    }
}

impl<BackendData: Backend> TouchTarget<AnvilState<BackendData>> for PointerFocusTarget {
    fn down(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &smithay::input::touch::DownEvent,
        seq: Serial,
    ) {
        self.inner_touch_target().down(seat, data, event, seq)
    }

    fn up(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &smithay::input::touch::UpEvent,
        seq: Serial,
    ) {
        self.inner_touch_target().up(seat, data, event, seq)
    }

    fn motion(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &smithay::input::touch::MotionEvent,
        seq: Serial,
    ) {
        self.inner_touch_target().motion(seat, data, event, seq)
    }

    fn frame(&self, seat: &Seat<AnvilState<BackendData>>, data: &mut AnvilState<BackendData>, seq: Serial) {
        self.inner_touch_target().frame(seat, data, seq)
    }

    fn cancel(&self, seat: &Seat<AnvilState<BackendData>>, data: &mut AnvilState<BackendData>, seq: Serial) {
        self.inner_touch_target().cancel(seat, data, seq)
    }

    fn shape(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &smithay::input::touch::ShapeEvent,
        seq: Serial,
    ) {
        self.inner_touch_target().shape(seat, data, event, seq)
    }

    fn orientation(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &smithay::input::touch::OrientationEvent,
        seq: Serial,
    ) {
        self.inner_touch_target().orientation(seat, data, event, seq)
    }
}

impl WaylandFocus for PointerFocusTarget {
    fn wl_surface(&self) -> Option<WlSurface> {
        match self {
            PointerFocusTarget::WlSurface(w) => w.wl_surface(),
            #[cfg(feature = "xwayland")]
            PointerFocusTarget::X11Surface(w) => w.wl_surface(),
            PointerFocusTarget::SSD(_) => None,
        }
    }
    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            PointerFocusTarget::WlSurface(w) => w.same_client_as(object_id),
            #[cfg(feature = "xwayland")]
            PointerFocusTarget::X11Surface(w) => w.same_client_as(object_id),
            PointerFocusTarget::SSD(w) => w
                .wl_surface()
                .map(|surface| surface.same_client_as(object_id))
                .unwrap_or(false),
        }
    }
}

impl WaylandFocus for KeyboardFocusTarget {
    fn wl_surface(&self) -> Option<WlSurface> {
        match self {
            KeyboardFocusTarget::Window(w) => w.wl_surface(),
            KeyboardFocusTarget::LayerSurface(l) => Some(l.wl_surface().clone()),
            KeyboardFocusTarget::Popup(p) => Some(p.wl_surface().clone()),
        }
    }
}

impl From<WlSurface> for PointerFocusTarget {
    fn from(value: WlSurface) -> Self {
        PointerFocusTarget::WlSurface(value)
    }
}

impl From<&WlSurface> for PointerFocusTarget {
    fn from(value: &WlSurface) -> Self {
        PointerFocusTarget::from(value.clone())
    }
}

impl From<PopupKind> for PointerFocusTarget {
    fn from(value: PopupKind) -> Self {
        PointerFocusTarget::from(value.wl_surface())
    }
}

#[cfg(feature = "xwayland")]
impl From<X11Surface> for PointerFocusTarget {
    fn from(value: X11Surface) -> Self {
        PointerFocusTarget::X11Surface(value)
    }
}

#[cfg(feature = "xwayland")]
impl From<&X11Surface> for PointerFocusTarget {
    fn from(value: &X11Surface) -> Self {
        PointerFocusTarget::from(value.clone())
    }
}

impl From<WindowElement> for KeyboardFocusTarget {
    fn from(w: WindowElement) -> Self {
        KeyboardFocusTarget::Window(w.0.clone())
    }
}

impl From<LayerSurface> for KeyboardFocusTarget {
    fn from(l: LayerSurface) -> Self {
        KeyboardFocusTarget::LayerSurface(l)
    }
}

impl From<PopupKind> for KeyboardFocusTarget {
    fn from(p: PopupKind) -> Self {
        KeyboardFocusTarget::Popup(p)
    }
}

impl From<KeyboardFocusTarget> for PointerFocusTarget {
    fn from(value: KeyboardFocusTarget) -> Self {
        match value {
            KeyboardFocusTarget::Window(w) => match w.underlying_surface() {
                WindowSurface::Wayland(w) => PointerFocusTarget::from(w.wl_surface()),
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(s) => PointerFocusTarget::from(s),
            },
            KeyboardFocusTarget::LayerSurface(surface) => PointerFocusTarget::from(surface.wl_surface()),
            KeyboardFocusTarget::Popup(popup) => PointerFocusTarget::from(popup.wl_surface()),
        }
    }
}
