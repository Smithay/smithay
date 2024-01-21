use std::path::PathBuf;

use winit::{
    dpi::PhysicalPosition,
    event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta},
};

use crate::backend::input::{
    self, AbsolutePositionEvent, Axis, AxisRelativeDirection, AxisSource, ButtonState, Device,
    DeviceCapability, Event, InputBackend, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    PointerMotionAbsoluteEvent, TouchCancelEvent, TouchDownEvent, TouchEvent, TouchMotionEvent, TouchSlot,
    TouchUpEvent, UnusedEvent,
};

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
    pub(crate) time: u64,
    pub(crate) key: u32,
    pub(crate) count: u32,
    pub(crate) state: ElementState,
}

impl Event<WinitInput> for WinitKeyboardInputEvent {
    fn time(&self) -> u64 {
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
    pub(crate) time: u64,
    pub(crate) position: RelativePosition,
    pub(crate) global_position: PhysicalPosition<f64>,
}

impl Event<WinitInput> for WinitMouseMovedEvent {
    fn time(&self) -> u64 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl PointerMotionAbsoluteEvent<WinitInput> for WinitMouseMovedEvent {}
impl AbsolutePositionEvent<WinitInput> for WinitMouseMovedEvent {
    fn x(&self) -> f64 {
        self.global_position.x
    }

    fn y(&self) -> f64 {
        self.global_position.y
    }

    fn x_transformed(&self, width: i32) -> f64 {
        f64::max(self.position.x * width as f64, 0.0)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        f64::max(self.position.y * height as f64, 0.0)
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerAxisEvent`]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WinitMouseWheelEvent {
    pub(crate) time: u64,
    pub(crate) delta: MouseScrollDelta,
}

impl Event<WinitInput> for WinitMouseWheelEvent {
    fn time(&self) -> u64 {
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

    // TODO: Use high-res scroll where backend supports it
    fn amount_v120(&self, axis: Axis) -> Option<f64> {
        match (axis, self.delta) {
            (Axis::Horizontal, MouseScrollDelta::LineDelta(x, _)) => Some(x as f64 * 120.),
            (Axis::Vertical, MouseScrollDelta::LineDelta(_, y)) => Some(y as f64 * 120.),
            (_, MouseScrollDelta::PixelDelta(_)) => None,
        }
    }

    // TODO: Implement with Wayland if `wl_pointer` version >= 9
    fn relative_direction(&self, _axis: Axis) -> AxisRelativeDirection {
        AxisRelativeDirection::Identical
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`PointerButtonEvent`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinitMouseInputEvent {
    pub(crate) time: u64,
    pub(crate) button: WinitMouseButton,
    pub(crate) state: ElementState,
    pub(crate) is_x11: bool,
}

impl Event<WinitInput> for WinitMouseInputEvent {
    fn time(&self) -> u64 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl PointerButtonEvent<WinitInput> for WinitMouseInputEvent {
    fn button_code(&self) -> u32 {
        match self.button {
            WinitMouseButton::Left => 0x110,
            WinitMouseButton::Right => 0x111,
            WinitMouseButton::Middle => 0x112,
            WinitMouseButton::Forward => 0x115,
            WinitMouseButton::Back => 0x116,
            WinitMouseButton::Other(b) => {
                if self.is_x11 {
                    input::xorg_mouse_to_libinput(b as u32)
                } else {
                    b as u32
                }
            }
        }
    }

    fn state(&self) -> ButtonState {
        self.state.into()
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchDownEvent`]
#[derive(Debug, Clone)]
pub struct WinitTouchStartedEvent {
    pub(crate) time: u64,
    pub(crate) position: RelativePosition,
    pub(crate) global_position: PhysicalPosition<f64>,
    pub(crate) id: u64,
}

impl Event<WinitInput> for WinitTouchStartedEvent {
    fn time(&self) -> u64 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl TouchDownEvent<WinitInput> for WinitTouchStartedEvent {}

impl TouchEvent<WinitInput> for WinitTouchStartedEvent {
    fn slot(&self) -> TouchSlot {
        Some(self.id as u32).into()
    }
}

impl AbsolutePositionEvent<WinitInput> for WinitTouchStartedEvent {
    fn x(&self) -> f64 {
        self.global_position.x
    }

    fn y(&self) -> f64 {
        self.global_position.y
    }

    fn x_transformed(&self, width: i32) -> f64 {
        f64::max(self.position.x * width as f64, 0.0)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        f64::max(self.position.y * height as f64, 0.0)
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchMotionEvent`]
#[derive(Debug, Clone)]
pub struct WinitTouchMovedEvent {
    pub(crate) time: u64,
    pub(crate) position: RelativePosition,
    pub(crate) global_position: PhysicalPosition<f64>,
    pub(crate) id: u64,
}

impl Event<WinitInput> for WinitTouchMovedEvent {
    fn time(&self) -> u64 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl TouchMotionEvent<WinitInput> for WinitTouchMovedEvent {}

impl TouchEvent<WinitInput> for WinitTouchMovedEvent {
    fn slot(&self) -> TouchSlot {
        Some(self.id as u32).into()
    }
}

impl AbsolutePositionEvent<WinitInput> for WinitTouchMovedEvent {
    fn x(&self) -> f64 {
        self.global_position.x
    }

    fn y(&self) -> f64 {
        self.global_position.y
    }

    fn x_transformed(&self, width: i32) -> f64 {
        f64::max(self.position.x * width as f64, 0.0)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        f64::max(self.position.y * height as f64, 0.0)
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a `TouchUpEvent`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinitTouchEndedEvent {
    pub(crate) time: u64,
    pub(crate) id: u64,
}

impl Event<WinitInput> for WinitTouchEndedEvent {
    fn time(&self) -> u64 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl TouchUpEvent<WinitInput> for WinitTouchEndedEvent {}

impl TouchEvent<WinitInput> for WinitTouchEndedEvent {
    fn slot(&self) -> TouchSlot {
        Some(self.id as u32).into()
    }
}

/// Winit-Backend internal event wrapping `winit`'s types into a [`TouchCancelEvent`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinitTouchCancelledEvent {
    pub(crate) time: u64,
    pub(crate) id: u64,
}

impl Event<WinitInput> for WinitTouchCancelledEvent {
    fn time(&self) -> u64 {
        self.time
    }

    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}

impl TouchCancelEvent<WinitInput> for WinitTouchCancelledEvent {}

impl TouchEvent<WinitInput> for WinitTouchCancelledEvent {
    fn slot(&self) -> TouchSlot {
        Some(self.id as u32).into()
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

/// Position relative to the source window, so each coordinate lays inside
/// the range from [0;1].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RelativePosition {
    /// Position of the `x` relative to the window.
    x: f64,
    /// Position of the `y` relative to the window.
    y: f64,
}

impl RelativePosition {
    pub(crate) fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

impl InputBackend for WinitInput {
    type Device = WinitVirtualDevice;
    type KeyboardKeyEvent = WinitKeyboardInputEvent;
    type PointerAxisEvent = WinitMouseWheelEvent;
    type PointerButtonEvent = WinitMouseInputEvent;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = WinitMouseMovedEvent;

    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;

    type TouchDownEvent = WinitTouchStartedEvent;
    type TouchUpEvent = WinitTouchEndedEvent;
    type TouchMotionEvent = WinitTouchMovedEvent;
    type TouchCancelEvent = WinitTouchCancelledEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;

    type SwitchToggleEvent = UnusedEvent;

    type SpecialEvent = UnusedEvent;
}
