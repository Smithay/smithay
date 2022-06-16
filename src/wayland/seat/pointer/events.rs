use wayland_server::protocol::{
    wl_pointer::{Axis, AxisSource, ButtonState},
    wl_surface::WlSurface,
};

use crate::{
    utils::{Logical, Point},
    wayland::Serial,
};

/// Pointer motion event
#[derive(Debug, Clone)]
pub struct MotionEvent {
    /// Location of the pointer in compositor space
    pub location: Point<f64, Logical>,
    /// Currently focused surface
    pub focus: Option<(WlSurface, Point<i32, Logical>)>,
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp of the event, with millisecond granularity
    pub time: u32,
}

/// Pointer button event

/// Mouse button click and release notifications.
/// The location of the click is given by the last motion or enter event.
#[derive(Debug, Clone, Copy)]
pub struct ButtonEvent {
    /// Serial of the event
    pub serial: Serial,
    /// Timestamp with millisecond granularity, with an undefined base.
    pub time: u32,
    /// Button that produced the event
    ///
    /// The button is a button code as defined in the
    /// Linux kernel's linux/input-event-codes.h header file, e.g. BTN_LEFT.
    ///
    /// Any 16-bit button code value is reserved for future additions to the kernel's event code list. All other button codes above 0xFFFF are currently undefined but may be used in future versions of this protocol.
    pub button: u32,
    /// Physical state of the button
    pub state: ButtonState,
}

/// A frame of pointer axis events.
///
/// Can be used with the builder pattern, e.g.:
///
/// ```ignore
/// AxisFrame::new()
///     .source(AxisSource::Wheel)
///     .discrete(Axis::Vertical, 6)
///     .value(Axis::Vertical, 30, time)
///     .stop(Axis::Vertical);
/// ```
#[derive(Copy, Clone, Debug)]
pub struct AxisFrame {
    pub(super) source: Option<AxisSource>,
    pub(super) time: u32,
    pub(super) axis: (f64, f64),
    pub(super) discrete: (i32, i32),
    pub(super) stop: (bool, bool),
}

impl AxisFrame {
    /// Create a new frame of axis events
    pub fn new(time: u32) -> Self {
        AxisFrame {
            source: None,
            time,
            axis: (0.0, 0.0),
            discrete: (0, 0),
            stop: (false, false),
        }
    }

    /// Specify the source of the axis events
    ///
    /// This event is optional, if no source is known, you can ignore this call.
    /// Only one source event is allowed per frame.
    ///
    /// Using the [`AxisSource::Finger`] requires a stop event to be send,
    /// when the user lifts off the finger (not necessarily in the same frame).
    pub fn source(mut self, source: AxisSource) -> Self {
        self.source = Some(source);
        self
    }

    /// Specify discrete scrolling steps additionally to the computed value.
    ///
    /// This event is optional and gives the client additional information about
    /// the nature of the axis event. E.g. a scroll wheel might issue separate steps,
    /// while a touchpad may never issue this event as it has no steps.
    pub fn discrete(mut self, axis: Axis, steps: i32) -> Self {
        match axis {
            Axis::HorizontalScroll => {
                self.discrete.0 = steps;
            }
            Axis::VerticalScroll => {
                self.discrete.1 = steps;
            }
            _ => unreachable!(),
        };
        self
    }

    /// The actual scroll value. This event is the only required one, but can also
    /// be send multiple times. The values off one frame will be accumulated by the client.
    pub fn value(mut self, axis: Axis, value: f64) -> Self {
        match axis {
            Axis::HorizontalScroll => {
                self.axis.0 = value;
            }
            Axis::VerticalScroll => {
                self.axis.1 = value;
            }
            _ => unreachable!(),
        };
        self
    }

    /// Notification of stop of scrolling on an axis.
    ///
    /// This event is required for sources of the [`AxisSource::Finger`] type
    /// and otherwise optional.
    pub fn stop(mut self, axis: Axis) -> Self {
        match axis {
            Axis::HorizontalScroll => {
                self.stop.0 = true;
            }
            Axis::VerticalScroll => {
                self.stop.1 = true;
            }
            _ => unreachable!(),
        };
        self
    }
}
