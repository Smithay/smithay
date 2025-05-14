use crate::backend::input::{
    self as backend, TabletToolCapabilities, TabletToolDescriptor, TabletToolTipState, TabletToolType,
};

use input as libinput;
use input::event;
use input::event::{tablet_tool, EventTrait};

use super::LibinputInputBackend;

/// Marker for tablet tool events
pub trait IsTabletEvent: tablet_tool::TabletToolEventTrait + EventTrait {}

impl IsTabletEvent for tablet_tool::TabletToolAxisEvent {}
impl IsTabletEvent for tablet_tool::TabletToolProximityEvent {}
impl IsTabletEvent for tablet_tool::TabletToolTipEvent {}
impl IsTabletEvent for tablet_tool::TabletToolButtonEvent {}

impl<E> backend::Event<LibinputInputBackend> for E
where
    E: IsTabletEvent,
{
    fn time(&self) -> u64 {
        tablet_tool::TabletToolEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::TabletToolAxisEvent<LibinputInputBackend> for tablet_tool::TabletToolAxisEvent {}

impl backend::TabletToolProximityEvent<LibinputInputBackend> for tablet_tool::TabletToolProximityEvent {
    fn state(&self) -> backend::ProximityState {
        match tablet_tool::TabletToolProximityEvent::proximity_state(self) {
            tablet_tool::ProximityState::In => backend::ProximityState::In,
            tablet_tool::ProximityState::Out => backend::ProximityState::Out,
        }
    }
}

impl backend::TabletToolTipEvent<LibinputInputBackend> for tablet_tool::TabletToolTipEvent {
    fn tip_state(&self) -> TabletToolTipState {
        match tablet_tool::TabletToolTipEvent::tip_state(self) {
            tablet_tool::TipState::Up => backend::TabletToolTipState::Up,
            tablet_tool::TipState::Down => backend::TabletToolTipState::Down,
        }
    }
}

impl<E> backend::AbsolutePositionEvent<LibinputInputBackend> for E
where
    E: IsTabletEvent + event::EventTrait,
{
    fn x(&self) -> f64 {
        tablet_tool::TabletToolEventTrait::x(self)
    }

    fn y(&self) -> f64 {
        tablet_tool::TabletToolEventTrait::y(self)
    }

    fn x_transformed(&self, width: i32) -> f64 {
        tablet_tool::TabletToolEventTrait::x_transformed(self, width as u32)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        tablet_tool::TabletToolEventTrait::y_transformed(self, height as u32)
    }
}

impl<E> backend::TabletToolEvent<LibinputInputBackend> for E
where
    E: IsTabletEvent + event::EventTrait,
{
    fn tool(&self) -> TabletToolDescriptor {
        let tool = self.tool();

        let tool_type = match tool.tool_type() {
            Some(tablet_tool::TabletToolType::Pen) => TabletToolType::Pen,
            Some(tablet_tool::TabletToolType::Eraser) => TabletToolType::Eraser,
            Some(tablet_tool::TabletToolType::Brush) => TabletToolType::Brush,
            Some(tablet_tool::TabletToolType::Pencil) => TabletToolType::Pencil,
            Some(tablet_tool::TabletToolType::Airbrush) => TabletToolType::Airbrush,
            Some(tablet_tool::TabletToolType::Mouse) => TabletToolType::Mouse,
            Some(tablet_tool::TabletToolType::Lens) => TabletToolType::Lens,
            Some(tablet_tool::TabletToolType::Totem) => TabletToolType::Totem,
            _ => TabletToolType::Unknown,
        };

        let hardware_serial = tool.serial();
        let hardware_id_wacom = tool.tool_id();

        let mut capabilities = TabletToolCapabilities::empty();

        capabilities.set(TabletToolCapabilities::TILT, tool.has_tilt());
        capabilities.set(TabletToolCapabilities::PRESSURE, tool.has_pressure());
        capabilities.set(TabletToolCapabilities::DISTANCE, tool.has_distance());
        capabilities.set(TabletToolCapabilities::ROTATION, tool.has_rotation());
        capabilities.set(TabletToolCapabilities::SLIDER, tool.has_slider());
        capabilities.set(TabletToolCapabilities::WHEEL, tool.has_wheel());

        TabletToolDescriptor {
            tool_type,
            hardware_serial,
            hardware_id_wacom,
            capabilities,
        }
    }

    fn delta_x(&self) -> f64 {
        tablet_tool::TabletToolEventTrait::dx(self)
    }

    fn delta_y(&self) -> f64 {
        tablet_tool::TabletToolEventTrait::dy(self)
    }

    fn distance(&self) -> f64 {
        tablet_tool::TabletToolEventTrait::distance(self)
    }

    fn distance_has_changed(&self) -> bool {
        tablet_tool::TabletToolEventTrait::distance_has_changed(self)
    }

    fn pressure(&self) -> f64 {
        tablet_tool::TabletToolEventTrait::pressure(self)
    }

    fn pressure_has_changed(&self) -> bool {
        tablet_tool::TabletToolEventTrait::pressure_has_changed(self)
    }

    fn slider_position(&self) -> f64 {
        tablet_tool::TabletToolEventTrait::slider_position(self)
    }

    fn slider_has_changed(&self) -> bool {
        tablet_tool::TabletToolEventTrait::slider_has_changed(self)
    }

    fn tilt_x(&self) -> f64 {
        tablet_tool::TabletToolEventTrait::tilt_x(self)
    }

    fn tilt_x_has_changed(&self) -> bool {
        tablet_tool::TabletToolEventTrait::tilt_x_has_changed(self)
    }

    fn tilt_y(&self) -> f64 {
        tablet_tool::TabletToolEventTrait::tilt_y(self)
    }

    fn tilt_y_has_changed(&self) -> bool {
        tablet_tool::TabletToolEventTrait::tilt_y_has_changed(self)
    }

    fn rotation(&self) -> f64 {
        tablet_tool::TabletToolEventTrait::rotation(self)
    }

    fn rotation_has_changed(&self) -> bool {
        tablet_tool::TabletToolEventTrait::rotation_has_changed(self)
    }

    fn wheel_delta(&self) -> f64 {
        tablet_tool::TabletToolEventTrait::wheel_delta(self)
    }

    fn wheel_delta_discrete(&self) -> i32 {
        // I have no idea why f64 is returend by this fn, in libinput's api wheel clicks are always i32
        tablet_tool::TabletToolEventTrait::wheel_delta_discrete(self) as i32
    }

    fn wheel_has_changed(&self) -> bool {
        tablet_tool::TabletToolEventTrait::wheel_has_changed(self)
    }
}

impl backend::TabletToolButtonEvent<LibinputInputBackend> for tablet_tool::TabletToolButtonEvent {
    fn button(&self) -> u32 {
        tablet_tool::TabletToolButtonEvent::button(self)
    }

    fn seat_button_count(&self) -> u32 {
        tablet_tool::TabletToolButtonEvent::seat_button_count(self)
    }

    fn button_state(&self) -> backend::ButtonState {
        tablet_tool::TabletToolButtonEvent::button_state(self).into()
    }
}
