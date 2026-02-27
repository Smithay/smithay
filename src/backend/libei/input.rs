// Implementation of `InputBackend` for reis requests

use reis::{
    eis,
    request::{self, DeviceCapability},
};
use std::path::PathBuf;

use crate::backend::input::{self, InputBackend};

use super::EiInput;

/// Generic ei scroll event
#[derive(Debug)]
pub enum ScrollEvent {
    /// Continuous scroll event
    Delta(request::ScrollDelta),
    /// Scroll cancel event
    Cancel(request::ScrollCancel),
    /// Discrete scroll event
    Discrete(request::ScrollDiscrete),
    /// Scroll stop event
    Stop(request::ScrollStop),
}

impl InputBackend for EiInput {
    type Device = request::Device;
    type KeyboardKeyEvent = request::KeyboardKey;
    type PointerAxisEvent = ScrollEvent;
    type PointerButtonEvent = request::Button;
    type PointerMotionEvent = request::PointerMotion;
    type PointerMotionAbsoluteEvent = request::PointerMotionAbsolute;

    type GestureSwipeBeginEvent = input::UnusedEvent;
    type GestureSwipeUpdateEvent = input::UnusedEvent;
    type GestureSwipeEndEvent = input::UnusedEvent;
    type GesturePinchBeginEvent = input::UnusedEvent;
    type GesturePinchUpdateEvent = input::UnusedEvent;
    type GesturePinchEndEvent = input::UnusedEvent;
    type GestureHoldBeginEvent = input::UnusedEvent;
    type GestureHoldEndEvent = input::UnusedEvent;

    type TouchDownEvent = request::TouchDown;
    type TouchUpEvent = request::TouchUp;
    type TouchMotionEvent = request::TouchMotion;
    type TouchCancelEvent = request::TouchCancel;
    type TouchFrameEvent = input::UnusedEvent;

    type TabletToolAxisEvent = input::UnusedEvent;
    type TabletToolProximityEvent = input::UnusedEvent;
    type TabletToolTipEvent = input::UnusedEvent;
    type TabletToolButtonEvent = input::UnusedEvent;

    type SwitchToggleEvent = input::UnusedEvent;

    type SpecialEvent = input::UnusedEvent;
}

impl input::Device for request::Device {
    fn id(&self) -> String {
        use reis::Interface;
        use rustix::fd::{AsFd, AsRawFd};
        let object = self.device().as_object();
        let id = object.id();
        // XXX don't panic?
        // don't need to be valid after calloop source is destroyed?
        // - (source keeps backend open, so weak handle upgrades)
        let fd = object.backend().unwrap().as_fd().as_raw_fd();
        format!("ei-{fd}-{id}")
    }

    fn name(&self) -> String {
        self.name().unwrap_or("").to_string()
    }

    fn has_capability(&self, capability: input::DeviceCapability) -> bool {
        match capability {
            input::DeviceCapability::Gesture => false,
            input::DeviceCapability::Keyboard => self.has_capability(DeviceCapability::Keyboard),
            input::DeviceCapability::Pointer => {
                self.has_capability(DeviceCapability::Pointer)
                    || self.has_capability(DeviceCapability::PointerAbsolute)
            }
            input::DeviceCapability::Switch => false,
            input::DeviceCapability::TabletPad => false,
            input::DeviceCapability::TabletTool => false,
            input::DeviceCapability::Touch => self.has_capability(DeviceCapability::Touch),
        }
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }

    fn syspath(&self) -> Option<PathBuf> {
        None
    }
}

impl<T: request::DeviceEvent + request::EventTime> input::Event<EiInput> for T {
    fn time(&self) -> u64 {
        request::EventTime::time(self)
    }

    fn device(&self) -> request::Device {
        request::DeviceEvent::device(self).clone()
    }
}

impl input::KeyboardKeyEvent<EiInput> for request::KeyboardKey {
    fn key_code(&self) -> input::Keycode {
        input::Keycode::from(self.key + 8)
    }

    fn state(&self) -> input::KeyState {
        match self.state {
            eis::keyboard::KeyState::Released => input::KeyState::Released,
            eis::keyboard::KeyState::Press => input::KeyState::Pressed,
        }
    }

    fn count(&self) -> u32 {
        1
    }
}

impl input::Event<EiInput> for ScrollEvent {
    fn time(&self) -> u64 {
        match self {
            Self::Delta(evt) => evt.time(),
            Self::Cancel(evt) => evt.time(),
            Self::Discrete(evt) => evt.time(),
            Self::Stop(evt) => evt.time(),
        }
    }

    fn device(&self) -> request::Device {
        match self {
            Self::Delta(evt) => evt.device(),
            Self::Cancel(evt) => evt.device(),
            Self::Discrete(evt) => evt.device(),
            Self::Stop(evt) => evt.device(),
        }
    }
}

impl input::PointerAxisEvent<EiInput> for ScrollEvent {
    fn amount(&self, axis: input::Axis) -> Option<f64> {
        match self {
            Self::Delta(evt) => match axis {
                input::Axis::Horizontal if evt.dx != 0.0 => Some(evt.dx.into()),
                input::Axis::Vertical if evt.dy != 0.0 => Some(evt.dy.into()),
                _ => None,
            },
            // Same as Mutter
            Self::Cancel(evt) => match axis {
                input::Axis::Horizontal if evt.x => Some(0.01),
                input::Axis::Vertical if evt.y => Some(0.01),
                _ => None,
            },
            Self::Discrete(_evt) => None,
            Self::Stop(evt) => match axis {
                input::Axis::Horizontal if evt.x => Some(0.0),
                input::Axis::Vertical if evt.y => Some(0.0),
                _ => None,
            },
        }
    }

    fn amount_v120(&self, axis: input::Axis) -> Option<f64> {
        match self {
            Self::Discrete(evt) => match axis {
                input::Axis::Horizontal if evt.discrete_dx != 0 => Some(evt.discrete_dx.into()),
                input::Axis::Vertical if evt.discrete_dy != 0 => Some(evt.discrete_dy.into()),
                _ => None,
            },
            _ => None,
        }
    }

    fn source(&self) -> input::AxisSource {
        // Mutter seems to also use wheel for all the scroll events
        input::AxisSource::Wheel
    }

    fn relative_direction(&self, _axis: input::Axis) -> input::AxisRelativeDirection {
        input::AxisRelativeDirection::Identical
    }
}

impl input::PointerButtonEvent<EiInput> for request::Button {
    fn button_code(&self) -> u32 {
        self.button
    }

    fn state(&self) -> input::ButtonState {
        match self.state {
            eis::button::ButtonState::Press => input::ButtonState::Pressed,
            eis::button::ButtonState::Released => input::ButtonState::Released,
        }
    }
}

impl input::PointerMotionEvent<EiInput> for request::PointerMotion {
    fn delta_x(&self) -> f64 {
        self.dx.into()
    }

    fn delta_y(&self) -> f64 {
        self.dy.into()
    }

    fn delta_x_unaccel(&self) -> f64 {
        self.dx.into()
    }

    fn delta_y_unaccel(&self) -> f64 {
        self.dy.into()
    }
}

impl input::PointerMotionAbsoluteEvent<EiInput> for request::PointerMotionAbsolute {}
impl input::AbsolutePositionEvent<EiInput> for request::PointerMotionAbsolute {
    fn x(&self) -> f64 {
        self.dx_absolute.into()
    }

    fn y(&self) -> f64 {
        self.dy_absolute.into()
    }

    fn x_transformed(&self, _width: i32) -> f64 {
        // XXX ?
        self.dx_absolute.into()
    }

    fn y_transformed(&self, _height: i32) -> f64 {
        self.dy_absolute.into()
    }
}

impl input::TouchDownEvent<EiInput> for request::TouchDown {}
impl input::TouchEvent<EiInput> for request::TouchDown {
    fn slot(&self) -> input::TouchSlot {
        Some(self.touch_id).into()
    }
}
impl input::AbsolutePositionEvent<EiInput> for request::TouchDown {
    fn x(&self) -> f64 {
        self.x.into()
    }

    fn y(&self) -> f64 {
        self.y.into()
    }

    fn x_transformed(&self, _width: i32) -> f64 {
        // XXX ?
        self.x.into()
    }

    fn y_transformed(&self, _height: i32) -> f64 {
        self.y.into()
    }
}

impl input::TouchUpEvent<EiInput> for request::TouchUp {}
impl input::TouchEvent<EiInput> for request::TouchUp {
    fn slot(&self) -> input::TouchSlot {
        Some(self.touch_id).into()
    }
}

impl input::TouchMotionEvent<EiInput> for request::TouchMotion {}
impl input::TouchEvent<EiInput> for request::TouchMotion {
    fn slot(&self) -> input::TouchSlot {
        Some(self.touch_id).into()
    }
}
impl input::AbsolutePositionEvent<EiInput> for request::TouchMotion {
    fn x(&self) -> f64 {
        self.x.into()
    }

    fn y(&self) -> f64 {
        self.y.into()
    }

    fn x_transformed(&self, _width: i32) -> f64 {
        // XXX ?
        self.x.into()
    }

    fn y_transformed(&self, _height: i32) -> f64 {
        self.y.into()
    }
}

impl input::TouchCancelEvent<EiInput> for request::TouchCancel {}
impl input::TouchEvent<EiInput> for request::TouchCancel {
    fn slot(&self) -> input::TouchSlot {
        Some(self.touch_id).into()
    }
}
