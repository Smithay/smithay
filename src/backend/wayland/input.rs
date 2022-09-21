use sctk::{reexports::client::protocol::wl_pointer, seat::pointer::AxisScroll};

use crate::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Device, DeviceCapability, Event, InputBackend,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent,
    UnusedEvent,
};

use super::window::WindowId;

/// Marker used to define the [`InputBackend`] types for the Wayland backend.
#[derive(Debug)]
pub struct WaylandInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaylandVirtualDevice {
    pub(crate) capability: DeviceCapability,
}

impl Device for WaylandVirtualDevice {
    fn id(&self) -> String {
        "wayland".to_owned()
    }

    fn name(&self) -> String {
        match self.capability {
            DeviceCapability::Keyboard => "wayland virtual keyboard",
            DeviceCapability::Pointer => "wayland virtual pointer",
            DeviceCapability::Touch => "wayland virtual touch",

            _ => unreachable!("unimplemented wayland virtual device {:?}", self.capability),
        }
        .to_owned()
    }

    fn has_capability(&self, capability: DeviceCapability) -> bool {
        self.capability == capability
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }

    fn syspath(&self) -> Option<std::path::PathBuf> {
        None
    }
}

impl InputBackend for WaylandInput {
    type Device = WaylandVirtualDevice;

    type KeyboardKeyEvent = WaylandKeyboardKeyEvent;
    type PointerAxisEvent = WaylandPointerAxisEvent;
    type PointerButtonEvent = WaylandPointerButtonEvent;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = WaylandPointerMotionEvent;

    // Touch events could be supported in the future
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;

    // Tablet events could be supported in the future
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;

    type SpecialEvent = UnusedEvent;
}

#[derive(Debug)]
pub struct WaylandKeyboardKeyEvent {
    pub(crate) key_code: u32,
    pub(crate) state: KeyState,
    pub(crate) device: WaylandVirtualDevice,
    pub(crate) count: u32,
    pub(crate) time: u32,
    pub(crate) window: WindowId,
}

impl WaylandKeyboardKeyEvent {
    /// Returns the id of the window belonging to this event.
    pub fn window(&self) -> WindowId {
        self.window
    }
}

impl Event<WaylandInput> for WaylandKeyboardKeyEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WaylandVirtualDevice {
        self.device
    }
}

impl KeyboardKeyEvent<WaylandInput> for WaylandKeyboardKeyEvent {
    fn key_code(&self) -> u32 {
        self.key_code
    }

    fn state(&self) -> KeyState {
        self.state
    }

    fn count(&self) -> u32 {
        self.count
    }
}

/// Wayland-Backend internal event wrapping `Wayland`'s types into a [`PointerAxisEvent`]
#[derive(Debug)]
pub struct WaylandPointerAxisEvent {
    pub(crate) device: WaylandVirtualDevice,
    pub(crate) vertical: AxisScroll,
    pub(crate) horizontal: AxisScroll,
    pub(crate) source: Option<wl_pointer::AxisSource>,
    pub(crate) time: u32,
    pub(crate) window: WindowId,
}

impl WaylandPointerAxisEvent {
    /// Returns the id of the window belonging to this event.
    pub fn window(&self) -> WindowId {
        self.window
    }
}

impl Event<WaylandInput> for WaylandPointerAxisEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WaylandVirtualDevice {
        self.device
    }
}

impl PointerAxisEvent<WaylandInput> for WaylandPointerAxisEvent {
    fn amount(&self, axis: Axis) -> Option<f64> {
        let amount = match axis {
            Axis::Vertical => self.vertical.absolute,
            Axis::Horizontal => self.horizontal.absolute,
        };

        Some(amount)
    }

    fn amount_discrete(&self, axis: Axis) -> Option<f64> {
        let amount = match axis {
            Axis::Vertical => self.vertical.discrete,
            Axis::Horizontal => self.horizontal.discrete,
        };

        Some(amount as f64).filter(|&amount| amount == 0.0)
    }

    fn source(&self) -> AxisSource {
        match self.source {
            Some(wl_pointer::AxisSource::Continuous) => AxisSource::Continuous,
            Some(wl_pointer::AxisSource::Finger) => AxisSource::Finger,
            Some(wl_pointer::AxisSource::Wheel) => AxisSource::Wheel,
            Some(wl_pointer::AxisSource::WheelTilt) => AxisSource::WheelTilt,
            // FIXME: Wheel is probably wrong to represent "unknown source"
            _ => AxisSource::Wheel,
        }
    }
}

/// X11-Backend internal event wrapping `X11`'s types into a [`PointerButtonEvent`]
#[derive(Debug)]
pub struct WaylandPointerButtonEvent {
    pub(crate) button: u32,
    pub(crate) time: u32,
    pub(crate) state: ButtonState,
    pub(crate) window: WindowId,
    pub(crate) device: WaylandVirtualDevice,
}

impl WaylandPointerButtonEvent {
    /// Returns the id of the window belonging to this event.
    pub fn window(&self) -> WindowId {
        self.window
    }
}

impl Event<WaylandInput> for WaylandPointerButtonEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WaylandVirtualDevice {
        self.device
    }
}

impl PointerButtonEvent<WaylandInput> for WaylandPointerButtonEvent {
    fn button_code(&self) -> u32 {
        self.button
    }

    fn state(&self) -> ButtonState {
        self.state
    }
}

#[derive(Debug)]
pub struct WaylandPointerMotionEvent {
    pub(crate) time: u32,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) window: WindowId,
    pub(crate) device: WaylandVirtualDevice,
}

impl WaylandPointerMotionEvent {
    /// Returns the id of the window belonging to this event.
    pub fn window(&self) -> WindowId {
        self.window
    }
}

impl Event<WaylandInput> for WaylandPointerMotionEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WaylandVirtualDevice {
        self.device
    }
}

impl AbsolutePositionEvent<WaylandInput> for WaylandPointerMotionEvent {
    fn x(&self) -> f64 {
        self.x
    }

    fn y(&self) -> f64 {
        self.y
    }

    fn x_transformed(&self, _width: i32) -> f64 {
        self.x() // FIXME this is probably wrong
                 //f64::max(self.x * width as f64 / self.size.w as f64, 0.0)
    }

    fn y_transformed(&self, _height: i32) -> f64 {
        self.y() // FIXME this is probably wrong
                 //f64::max(self.y * height as f64 / self.size.h as f64, 0.0)
    }
}

impl PointerMotionAbsoluteEvent<WaylandInput> for WaylandPointerMotionEvent {}

impl From<Axis> for wl_pointer::Axis {
    fn from(axis: Axis) -> Self {
        match axis {
            Axis::Vertical => wl_pointer::Axis::VerticalScroll,
            Axis::Horizontal => wl_pointer::Axis::HorizontalScroll,
        }
    }
}
