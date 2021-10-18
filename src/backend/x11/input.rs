//! Input backend implementation for the X11 backend.

use super::X11Error;
use crate::{
    backend::input::{
        self, Axis, AxisSource, ButtonState, Device, DeviceCapability, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, MouseButton, PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent,
        UnusedEvent,
    },
    utils::{Logical, Size},
};

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
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct X11KeyboardInputEvent {
    pub(crate) time: u32,
    pub(crate) key: u32,
    pub(crate) count: u32,
    pub(crate) state: KeyState,
}

impl input::Event<X11Input> for X11KeyboardInputEvent {
    fn time(&self) -> u32 {
        self.time
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
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct X11MouseWheelEvent {
    pub(crate) time: u32,
    pub(crate) axis: Axis,
    pub(crate) amount: f64,
}

impl input::Event<X11Input> for X11MouseWheelEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl PointerAxisEvent<X11Input> for X11MouseWheelEvent {
    fn amount(&self, _axis: Axis) -> Option<f64> {
        None
    }

    fn amount_discrete(&self, axis: Axis) -> Option<f64> {
        if self.axis == axis {
            Some(self.amount)
        } else {
            Some(0.0)
        }
    }

    fn source(&self) -> AxisSource {
        // X11 seems to act within the scope of individual rachets of a scroll wheel.
        AxisSource::Wheel
    }
}

/// X11-Backend internal event wrapping `X11`'s types into a [`PointerButtonEvent`]
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct X11MouseInputEvent {
    pub(crate) time: u32,
    pub(crate) button: MouseButton,
    pub(crate) state: ButtonState,
}

impl input::Event<X11Input> for X11MouseInputEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl PointerButtonEvent<X11Input> for X11MouseInputEvent {
    fn button(&self) -> MouseButton {
        self.button
    }

    fn state(&self) -> ButtonState {
        self.state
    }
}

/// X11-Backend internal event wrapping `X11`'s types into a [`PointerMotionAbsoluteEvent`]
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct X11MouseMovedEvent {
    pub(crate) time: u32,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) size: Size<u16, Logical>,
}

impl input::Event<X11Input> for X11MouseMovedEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> X11VirtualDevice {
        X11VirtualDevice
    }
}

impl PointerMotionAbsoluteEvent<X11Input> for X11MouseMovedEvent {
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
    type EventError = X11Error;

    type Device = X11VirtualDevice;
    type KeyboardKeyEvent = X11KeyboardInputEvent;
    type PointerAxisEvent = X11MouseWheelEvent;
    type PointerButtonEvent = X11MouseInputEvent;

    type PointerMotionEvent = UnusedEvent;

    type PointerMotionAbsoluteEvent = X11MouseMovedEvent;

    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;

    type SpecialEvent = UnusedEvent;

    fn dispatch_new_events<F>(&mut self, _callback: F) -> Result<(), Self::EventError>
    where
        F: FnMut(InputEvent<Self>),
    {
        // The implementation of the trait here is exclusively for type definitions.
        // See `X11Event::Input` to handle input events.
        unreachable!()
    }
}
