use std::{cell::RefCell, path::PathBuf, rc::Rc};

use winit::{
    dpi::LogicalPosition,
    event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta},
};

use crate::backend::input::{
    Axis, AxisSource, ButtonState, Device, DeviceCapability, Event, InputBackend, InputEvent, KeyState,
    KeyboardKeyEvent, MouseButton, PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent,
    TouchCancelEvent, TouchDownEvent, TouchMotionEvent, TouchSlot, TouchUpEvent, UnusedEvent,
};

use super::{WindowSize, WinitError};

/// Marker used to define the `InputBackend` types for the winit backend.
#[derive(Debug)]
pub struct WinitInput;

/// Virtual input device used by the backend to associate input events
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct WinitVirtualDevice;

impl Device for WinitVirtualDevice {
    fn id(&self) -> String {
        String::from("winit")
    }

    fn name(&self) -> String {
        String::from("winit virtual input")
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

    fn syspath(&self) -> Option<PathBuf> {
        None
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`KeyboardKeyEvent`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinitKeyboardInputEvent {
    pub(crate) time: u32,
    pub(crate) key: u32,
    pub(crate) count: u32,
    pub(crate) state: ElementState,
}

impl Event<WinitInput> for WinitKeyboardInputEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl KeyboardKeyEvent<WinitInput> for WinitKeyboardInputEvent {
    fn key_code(&self) -> u32 {
        self.key
    }

    fn state(&self) -> KeyState {
        self.state.into()
    }

    fn count(&self) -> u32 {
        self.count
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerMotionAbsoluteEvent`]
#[derive(Debug, Clone)]
pub struct WinitMouseMovedEvent {
    pub(crate) size: Rc<RefCell<WindowSize>>,
    pub(crate) time: u32,
    pub(crate) logical_position: LogicalPosition<f64>,
}

impl Event<WinitInput> for WinitMouseMovedEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl PointerMotionAbsoluteEvent<WinitInput> for WinitMouseMovedEvent {
    // TODO: maybe use {Logical, Physical}Position from winit?
    fn x(&self) -> f64 {
        let wsize = self.size.borrow();
        self.logical_position.x * wsize.scale_factor
    }

    fn y(&self) -> f64 {
        let wsize = self.size.borrow();
        self.logical_position.y * wsize.scale_factor
    }

    fn x_transformed(&self, width: i32) -> f64 {
        let wsize = self.size.borrow();
        let w_width = wsize.logical_size().w;
        f64::max(self.logical_position.x * width as f64 / w_width, 0.0)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        let wsize = self.size.borrow();
        let w_height = wsize.logical_size().h;
        f64::max(self.logical_position.y * height as f64 / w_height, 0.0)
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerAxisEvent`]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WinitMouseWheelEvent {
    pub(crate) time: u32,
    pub(crate) delta: MouseScrollDelta,
}

impl Event<WinitInput> for WinitMouseWheelEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl PointerAxisEvent<WinitInput> for WinitMouseWheelEvent {
    fn source(&self) -> AxisSource {
        match self.delta {
            MouseScrollDelta::LineDelta(_, _) => AxisSource::Wheel,
            MouseScrollDelta::PixelDelta(_) => AxisSource::Continuous,
        }
    }

    fn amount(&self, axis: Axis) -> Option<f64> {
        match (axis, self.delta) {
            (Axis::Horizontal, MouseScrollDelta::PixelDelta(delta)) => Some(delta.x),
            (Axis::Vertical, MouseScrollDelta::PixelDelta(delta)) => Some(delta.y),
            (_, MouseScrollDelta::LineDelta(_, _)) => None,
        }
    }

    fn amount_discrete(&self, axis: Axis) -> Option<f64> {
        match (axis, self.delta) {
            (Axis::Horizontal, MouseScrollDelta::LineDelta(x, _)) => Some(x as f64),
            (Axis::Vertical, MouseScrollDelta::LineDelta(_, y)) => Some(y as f64),
            (_, MouseScrollDelta::PixelDelta(_)) => None,
        }
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerButtonEvent`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinitMouseInputEvent {
    pub(crate) time: u32,
    pub(crate) button: WinitMouseButton,
    pub(crate) state: ElementState,
}

impl Event<WinitInput> for WinitMouseInputEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl PointerButtonEvent<WinitInput> for WinitMouseInputEvent {
    fn button(&self) -> MouseButton {
        self.button.into()
    }

    fn state(&self) -> ButtonState {
        self.state.into()
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchDownEvent`]
#[derive(Debug, Clone)]
pub struct WinitTouchStartedEvent {
    pub(crate) size: Rc<RefCell<WindowSize>>,
    pub(crate) time: u32,
    pub(crate) location: LogicalPosition<f64>,
    pub(crate) id: u64,
}

impl Event<WinitInput> for WinitTouchStartedEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl TouchDownEvent<WinitInput> for WinitTouchStartedEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }

    fn x(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.x * wsize.scale_factor
    }

    fn y(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.y * wsize.scale_factor
    }

    fn x_transformed(&self, width: i32) -> f64 {
        let wsize = self.size.borrow();
        let w_width = wsize.logical_size().w;
        f64::max(self.location.x * width as f64 / w_width, 0.0)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        let wsize = self.size.borrow();
        let w_height = wsize.logical_size().h;
        f64::max(self.location.y * height as f64 / w_height, 0.0)
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchMotionEvent`]
#[derive(Debug, Clone)]
pub struct WinitTouchMovedEvent {
    pub(crate) size: Rc<RefCell<WindowSize>>,
    pub(crate) time: u32,
    pub(crate) location: LogicalPosition<f64>,
    pub(crate) id: u64,
}

impl Event<WinitInput> for WinitTouchMovedEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl TouchMotionEvent<WinitInput> for WinitTouchMovedEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }

    fn x(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.x * wsize.scale_factor
    }

    fn y(&self) -> f64 {
        let wsize = self.size.borrow();
        self.location.y * wsize.scale_factor
    }

    fn x_transformed(&self, width: i32) -> f64 {
        let wsize = self.size.borrow();
        let w_width = wsize.logical_size().w;
        f64::max(self.location.x * width as f64 / w_width, 0.0)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        let wsize = self.size.borrow();
        let w_height = wsize.logical_size().h;
        f64::max(self.location.y * height as f64 / w_height, 0.0)
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a `TouchUpEvent`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinitTouchEndedEvent {
    pub(crate) time: u32,
    pub(crate) id: u64,
}

impl Event<WinitInput> for WinitTouchEndedEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl TouchUpEvent<WinitInput> for WinitTouchEndedEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchCancelEvent`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinitTouchCancelledEvent {
    pub(crate) time: u32,
    pub(crate) id: u64,
}

impl Event<WinitInput> for WinitTouchCancelledEvent {
    fn time(&self) -> u32 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl TouchCancelEvent<WinitInput> for WinitTouchCancelledEvent {
    fn slot(&self) -> Option<TouchSlot> {
        Some(TouchSlot::new(self.id))
    }
}

impl From<WinitMouseButton> for MouseButton {
    fn from(button: WinitMouseButton) -> MouseButton {
        match button {
            WinitMouseButton::Left => MouseButton::Left,
            WinitMouseButton::Right => MouseButton::Right,
            WinitMouseButton::Middle => MouseButton::Middle,
            WinitMouseButton::Other(num) => MouseButton::Other(num as u8),
        }
    }
}

impl From<ElementState> for KeyState {
    fn from(state: ElementState) -> Self {
        match state {
            ElementState::Pressed => KeyState::Pressed,
            ElementState::Released => KeyState::Released,
        }
    }
}

impl From<ElementState> for ButtonState {
    fn from(state: ElementState) -> Self {
        match state {
            ElementState::Pressed => ButtonState::Pressed,
            ElementState::Released => ButtonState::Released,
        }
    }
}

impl InputBackend for WinitInput {
    type EventError = WinitError;

    type Device = WinitVirtualDevice;
    type KeyboardKeyEvent = WinitKeyboardInputEvent;
    type PointerAxisEvent = WinitMouseWheelEvent;
    type PointerButtonEvent = WinitMouseInputEvent;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = WinitMouseMovedEvent;
    type TouchDownEvent = WinitTouchStartedEvent;
    type TouchUpEvent = WinitTouchEndedEvent;
    type TouchMotionEvent = WinitTouchMovedEvent;
    type TouchCancelEvent = WinitTouchCancelledEvent;
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
        unreachable!()
    }
}
