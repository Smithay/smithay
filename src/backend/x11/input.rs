//! Input backend implementation for the X11 backend.

use super::{window_inner::WindowInner, Window, WindowTemporary};
use crate::{
    backend::input::{
        self, AbsolutePositionEvent, Axis, AxisRelativeDirection, AxisSource, ButtonState, Device,
        DeviceCapability, InputBackend, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        PointerMotionAbsoluteEvent, UnusedEvent,
    },
    utils::{Logical, Size},
};
use std::sync::Weak;

/// Marker used to define the `InputBackend` types for the X11 backend.
#[derive(Debug)]
pub struct X11Input;

/// Virtual input device used by the backend to associate input events.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct X11VirtualDevice;

impl Device for X11VirtualDevice {
    fn id(&self) -> String {
        "x11".to_owned()
    }

    fn name(&self) -> String {
        "x11 virtual input".to_owned()
    }

    fn has_capability(&self, capability: DeviceCapability) -> bool {
        matches!(
            capability,
            DeviceCapability::Keyboard | DeviceCapability::Pointer | DeviceCapability::Touch
        )
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }

    fn syspath(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// X11-Backend internal event wrapping `X11`'s types into a [`KeyboardKeyEvent`].
#[derive(Debug, Clone)]
pub struct X11KeyboardInputEvent {
    pub(crate) time: u32,
    pub(crate) key: u32,
    pub(crate) count: u32,
    pub(crate) state: KeyState,
    pub(crate) window: Weak<WindowInner>,
}

impl X11KeyboardInputEvent {
    /// Returns a temporary reference to the window belonging to this event.
    ///
    /// Returns None if the window is not alive anymore.
    pub fn window(&self) -> Option<impl AsRef<Window> + '_> {
        self.window.upgrade().map(Window).map(WindowTemporary)
    }
}

impl input::Event<X11Input> for X11KeyboardInputEvent {
    fn time(&self) -> u64 {
        self.time as u64 * 1000
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl KeyboardKeyEvent<X11Input> for X11KeyboardInputEvent {
    fn key_code(&self) -> u32 {
        self.key
    }

    fn state(&self) -> KeyState {
        self.state
    }

    fn count(&self) -> u32 {
        self.count
    }
}

/// X11-Backend internal event wrapping `X11`'s types into a [`PointerAxisEvent`]
#[derive(Debug, Clone)]
pub struct X11MouseWheelEvent {
    pub(crate) time: u32,
    pub(crate) axis: Axis,
    pub(crate) amount: f64,
    pub(crate) window: Weak<WindowInner>,
}

impl X11MouseWheelEvent {
    /// Returns a temporary reference to the window belonging to this event.
    ///
    /// Returns None if the window is not alive anymore.
    pub fn window(&self) -> Option<impl AsRef<Window> + '_> {
        self.window.upgrade().map(Window).map(WindowTemporary)
    }
}

impl input::Event<X11Input> for X11MouseWheelEvent {
    fn time(&self) -> u64 {
        self.time as u64 * 1000
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl PointerAxisEvent<X11Input> for X11MouseWheelEvent {
    fn amount(&self, _axis: Axis) -> Option<f64> {
        None
    }

    fn amount_v120(&self, axis: Axis) -> Option<f64> {
        if self.axis == axis {
            Some(self.amount * 120.)
        } else {
            Some(0.0)
        }
    }

    fn source(&self) -> AxisSource {
        // X11 seems to act within the scope of individual rachets of a scroll wheel.
        AxisSource::Wheel
    }

    fn relative_direction(&self, _axis: Axis) -> AxisRelativeDirection {
        AxisRelativeDirection::Identical
    }
}

/// X11-Backend internal event wrapping `X11`'s types into a [`PointerButtonEvent`]
#[derive(Debug, Clone)]
pub struct X11MouseInputEvent {
    pub(crate) time: u32,
    pub(crate) raw: u32,
    pub(crate) state: ButtonState,
    pub(crate) window: Weak<WindowInner>,
}

impl X11MouseInputEvent {
    /// Returns a temporary reference to the window belonging to this event.
    ///
    /// Returns None if the window is not alive anymore.
    pub fn window(&self) -> Option<impl AsRef<Window> + '_> {
        self.window.upgrade().map(Window).map(WindowTemporary)
    }
}

impl input::Event<X11Input> for X11MouseInputEvent {
    fn time(&self) -> u64 {
        self.time as u64 * 1000
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl PointerButtonEvent<X11Input> for X11MouseInputEvent {
    fn button_code(&self) -> u32 {
        input::xorg_mouse_to_libinput(self.raw)
    }

    fn state(&self) -> ButtonState {
        self.state
    }
}

/// X11-Backend internal event wrapping `X11`'s types into a [`PointerMotionAbsoluteEvent`]
#[derive(Debug, Clone)]
pub struct X11MouseMovedEvent {
    pub(crate) time: u32,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) size: Size<u16, Logical>,
    pub(crate) window: Weak<WindowInner>,
}

impl X11MouseMovedEvent {
    /// Returns a temporary reference to the window belonging to this event.
    ///
    /// Returns None if the window is not alive anymore.
    pub fn window(&self) -> Option<impl AsRef<Window> + '_> {
        self.window.upgrade().map(Window).map(WindowTemporary)
    }
}

impl input::Event<X11Input> for X11MouseMovedEvent {
    fn time(&self) -> u64 {
        self.time as u64 * 1000
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl PointerMotionAbsoluteEvent<X11Input> for X11MouseMovedEvent {}
impl AbsolutePositionEvent<X11Input> for X11MouseMovedEvent {
    fn x(&self) -> f64 {
        self.x
    }

    fn y(&self) -> f64 {
        self.y
    }

    fn x_transformed(&self, width: i32) -> f64 {
        f64::max(self.x * width as f64 / self.size.w as f64, 0.0)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        f64::max(self.y * height as f64 / self.size.h as f64, 0.0)
    }
}

impl InputBackend for X11Input {
    type Device = X11VirtualDevice;
    type KeyboardKeyEvent = X11KeyboardInputEvent;
    type PointerAxisEvent = X11MouseWheelEvent;
    type PointerButtonEvent = X11MouseInputEvent;

    type PointerMotionEvent = UnusedEvent;

    type PointerMotionAbsoluteEvent = X11MouseMovedEvent;

    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;

    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;

    type SwitchToggleEvent = UnusedEvent;

    type SpecialEvent = UnusedEvent;
}
