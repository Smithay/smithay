use super::{AbsolutePositionEvent, ButtonState, Event, InputBackend, UnusedEvent};
use crate::utils::{Logical, Point};
use bitflags::bitflags;

/// Description of physical tablet tool
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct TabletToolDescriptor {
    /// The tool type is the high-level type of the tool and usually decides the interaction expected from this tool.
    pub tool_type: TabletToolType,
    /// Unique hardware serial number of the tool
    pub hardware_serial: u64,
    /// Hardware id in Wacom’s format
    pub hardware_id_wacom: u64,
    /// Tool capabilities
    /// Notifies the client of any capabilities of this tool, beyond the main set of x/y axes and tip up/down detection
    pub capabilities: TabletToolCapabilities,
}

/// Describes the physical type of tool. The physical type of tool generally defines its base usage.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum TabletToolType {
    /// A generic pen.
    Pen,
    /// Eraser
    Eraser,
    /// A paintbrush-like tool.
    Brush,
    /// Physical drawing tool, e.g. Wacom Inking Pen
    Pencil,
    /// An airbrush-like tool.
    Airbrush,
    /// A mouse bound to the tablet.
    Mouse,
    /// A mouse tool with a lens.
    Lens,
    /// A rotary device with positional and rotation data.  
    Totem,
    /// Type of the device is not known or does not match any known ones
    Unknown,
}

bitflags! {
    /// Describes extra capabilities on a tablet.
    ///
    /// Any tool must provide x and y values, extra axes are device-specific.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct TabletToolCapabilities: u32 {
        /// Tilt axes
        const TILT = 1;
        /// Pressure axis
        const PRESSURE = 2;
        /// Distance axis
        const DISTANCE = 4;
        /// Z-rotation axis
        const ROTATION = 16;
        /// Slider axis
        const SLIDER = 32;
        /// Wheel axis
        const WHEEL = 64;
    }
}

/// Tablet tool event
///
/// The [AbsolutePositionEvent] implementation for tablet tool events produces (untransformed)
/// coordinates in mm from the top left corner of the tablet in its current logical orientation.
pub trait TabletToolEvent<B: InputBackend>: AbsolutePositionEvent<B> {
    /// Get tablet tool that caused this event
    fn tool(&self) -> TabletToolDescriptor;

    /// Delta between the last and new pointer device position interpreted as pixel movement
    fn delta(&self) -> Point<f64, Logical> {
        (self.delta_x(), self.delta_y()).into()
    }

    /// Returns the current tilt along the (X,Y) axis of the tablet's current logical
    /// orientation, in degrees off the tablet's z axis.
    ///
    /// That is, if the tool is perfectly orthogonal to the tablet, the tilt angle is 0.
    /// When the top tilts towards the logical top/left of the tablet, the x/y tilt
    /// angles are negative, if the top tilts towards the logical bottom/right of the
    /// tablet, the x/y tilt angles are positive.
    ///
    /// If this axis does not exist on the current tool, this function returns (0,0).
    fn tilt(&self) -> (f64, f64) {
        (self.tilt_x(), self.tilt_y())
    }

    /// Check if the tilt was updated in this event.
    fn tilt_has_changed(&self) -> bool {
        self.tilt_x_has_changed() || self.tilt_y_has_changed()
    }

    /// Delta on the x axis between the last and new pointer device position interpreted as pixel movement
    fn delta_x(&self) -> f64;
    /// Delta on the y axis between the last and new pointer device position interpreted as pixel movement
    fn delta_y(&self) -> f64;

    /// If this axis does not exist on the current tool, this function returns 0.
    fn distance(&self) -> f64;

    /// Check if the distance axis was updated in this event.
    fn distance_has_changed(&self) -> bool;

    /// Returns the current pressure being applied on the tool in use, normalized to the range [0, 1].
    ///
    /// If this axis does not exist on the current tool, this function returns 0.
    fn pressure(&self) -> f64;

    /// Check if the pressure axis was updated in this event.
    fn pressure_has_changed(&self) -> bool;

    /// Returns the current position of the slider on the tool, normalized to the range
    /// [-1, 1].
    ///
    /// The logical zero is the neutral position of the slider, or the logical center of
    /// the axis. This axis is available on e.g. the Wacom Airbrush.
    ///
    /// If this axis does not exist on the current tool, this function returns 0.
    fn slider_position(&self) -> f64;

    /// Check if the slider axis was updated in this event.
    fn slider_has_changed(&self) -> bool;

    /// Returns the current tilt along the X axis of the tablet's current logical
    /// orientation, in degrees off the tablet's z axis.
    ///
    /// That is, if the tool is perfectly orthogonal to the tablet, the tilt angle is 0.
    /// When the top tilts towards the logical top/left of the tablet, the x/y tilt
    /// angles are negative, if the top tilts towards the logical bottom/right of the
    /// tablet, the x/y tilt angles are positive.
    ///
    /// If this axis does not exist on the current tool, this function returns 0.
    fn tilt_x(&self) -> f64;

    /// Check if the tilt x axis was updated in this event.
    fn tilt_x_has_changed(&self) -> bool;

    /// Returns the current tilt along the Y axis of the tablet's current logical
    /// orientation, in degrees off the tablet's z axis.
    ///
    /// That is, if the tool is perfectly orthogonal to the tablet, the tilt angle is 0.
    /// When the top tilts towards the logical top/left of the tablet, the x/y tilt
    /// angles are negative, if the top tilts towards the logical bottom/right of the
    /// tablet, the x/y tilt angles are positive.
    ///
    /// If this axis does not exist on the current tool, this function returns 0.
    fn tilt_y(&self) -> f64;

    /// Check if the tilt y axis was updated in this event.
    fn tilt_y_has_changed(&self) -> bool;

    /// Returns the current z rotation of the tool in degrees, clockwise from the tool's logical neutral position.
    ///
    /// For tools of type Mouse and Lens the logical neutral position is pointing to the current logical north of the tablet.
    /// For tools of type Brush, the logical neutral position is with the buttons pointing up.
    ///
    /// If this axis does not exist on the current tool, this function returns 0.
    fn rotation(&self) -> f64;

    /// Check if the z-rotation axis was updated in this event.
    fn rotation_has_changed(&self) -> bool;

    /// Return the delta for the wheel in degrees.
    fn wheel_delta(&self) -> f64;
    /// Return the delta for the wheel in discrete steps (e.g. wheel clicks).
    fn wheel_delta_discrete(&self) -> i32;
    /// Check if the wheel axis was updated in this event.
    fn wheel_has_changed(&self) -> bool;
}

impl<B: InputBackend> TabletToolEvent<B> for UnusedEvent {
    fn tool(&self) -> TabletToolDescriptor {
        match *self {}
    }
    fn delta_x(&self) -> f64 {
        match *self {}
    }
    fn delta_y(&self) -> f64 {
        match *self {}
    }
    fn distance(&self) -> f64 {
        match *self {}
    }
    fn distance_has_changed(&self) -> bool {
        match *self {}
    }
    fn pressure(&self) -> f64 {
        match *self {}
    }
    fn pressure_has_changed(&self) -> bool {
        match *self {}
    }
    fn slider_position(&self) -> f64 {
        match *self {}
    }
    fn slider_has_changed(&self) -> bool {
        match *self {}
    }
    fn tilt_x(&self) -> f64 {
        match *self {}
    }
    fn tilt_x_has_changed(&self) -> bool {
        match *self {}
    }
    fn tilt_y(&self) -> f64 {
        match *self {}
    }
    fn tilt_y_has_changed(&self) -> bool {
        match *self {}
    }
    fn rotation(&self) -> f64 {
        match *self {}
    }
    fn rotation_has_changed(&self) -> bool {
        match *self {}
    }
    fn wheel_delta(&self) -> f64 {
        match *self {}
    }
    fn wheel_delta_discrete(&self) -> i32 {
        match *self {}
    }
    fn wheel_has_changed(&self) -> bool {
        match *self {}
    }
}

/// Trait for axis tablet tool events.
pub trait TabletToolAxisEvent<B: InputBackend>: TabletToolEvent<B> + Event<B> {}

impl<B: InputBackend> TabletToolAxisEvent<B> for UnusedEvent {}

/// The state of proximity for a tool on a device.
///
/// The proximity of a tool is a binary state signalling whether the tool is within a
/// detectable distance of the tablet device. A tool that is out of proximity cannot
/// generate events.
///
/// On some hardware a tool goes out of proximity when it ceases to touch the surface. On
/// other hardware, the tool is still detectable within a short distance (a few cm) off
/// the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProximityState {
    /// Out of proximity
    Out,
    /// In proximity
    In,
}

/// Trait for tablet tool proximity events.
pub trait TabletToolProximityEvent<B: InputBackend>: TabletToolEvent<B> + Event<B> {
    /// Returns the new proximity state of a tool from a proximity event.
    ///
    /// Used to check whether or not a tool came in or out of proximity during an
    /// `TabletToolProximityEvent`.
    ///
    /// See [Handling of proximity events](https://wayland.freedesktop.org/libinput/doc/latest/tablet-support.html#tablet-fake-proximity)
    /// for recommendations on proximity handling.
    fn state(&self) -> ProximityState;
}

impl<B: InputBackend> TabletToolProximityEvent<B> for UnusedEvent {
    fn state(&self) -> ProximityState {
        match *self {}
    }
}

/// The tip contact state for a tool on a device.
///
/// The tip contact state of a tool is a binary state signalling whether the tool is
/// touching the surface of the tablet device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabletToolTipState {
    /// Not touching the surface
    Up,
    /// Touching the surface
    Down,
}

/// Signals that a tool has come in contact with the surface of a device with the
/// `DeviceCapability::TabletTool` capability.
///
/// On devices without distance proximity detection, the `TabletToolTipEvent` is sent
/// immediately after `TabletToolProximityEvent` for the tip down event, and
/// immediately before for the tip up event.
///
/// The decision when a tip touches the surface is device-dependent and may be
/// derived from pressure data or other means. If the tip state is changed by axes
/// changing state, the `TabletToolTipEvent` includes the changed axes and no
/// additional axis event is sent for this state change. In other words, a caller
/// must look at both `TabletToolAxisEvent` and `TabletToolTipEvent` events to know
/// the current state of the axes.
///
/// If a button state change occurs at the same time as a tip state change, the order
/// of events is device-dependent.
pub trait TabletToolTipEvent<B: InputBackend>: TabletToolEvent<B> + Event<B> {
    /// Returns the new tip state of a tool from a tip event.
    ///
    /// Used to check whether or not a tool came in contact with the tablet surface or
    /// left contact with the tablet surface during an `TabletToolTipEvent`.
    fn tip_state(&self) -> TabletToolTipState;
}

impl<B: InputBackend> TabletToolTipEvent<B> for UnusedEvent {
    fn tip_state(&self) -> TabletToolTipState {
        match *self {}
    }
}

/// Signals that a tool has changed a logical button state on a device with the DeviceCapability::TabletTool capability.
pub trait TabletToolButtonEvent<B: InputBackend>: TabletToolEvent<B> + Event<B> {
    /// Return the button that triggered this event.
    fn button(&self) -> u32;

    /// For the button of a TabletToolButtonEvent,
    /// return the total number of buttons pressed on all devices on the associated seat after the the event was triggered.
    fn seat_button_count(&self) -> u32;

    /// Return the button state of the event.
    fn button_state(&self) -> ButtonState;
}

impl<B: InputBackend> TabletToolButtonEvent<B> for UnusedEvent {
    fn button(&self) -> u32 {
        match *self {}
    }

    fn seat_button_count(&self) -> u32 {
        match *self {}
    }

    fn button_state(&self) -> ButtonState {
        match *self {}
    }
}
