//! Common traits for input backends to receive input from.

use backend::{SeatInternal, TouchSlotInternal};

use std::error::Error;
use std::hash::Hash;

/// A seat describes a group of input devices and at least one
/// graphics device belonging together.
///
/// By default only one seat exists for most systems and smithay backends
/// however multiseat configurations are possible and should be treated as
/// separated users, all with their own focus, input and cursor available.
///
/// Seats referring to the same internal id will always be equal and result in the same
/// hash, but capabilities of cloned and copied `Seat`s will not be updated by smithay.
/// Always referr to the `Seat` given by a callback for up-to-date information. You may
/// use this to calculate the differences since the last callback.
#[derive(Debug, Clone, Copy, Eq)]
pub struct Seat {
    id: u64,
    capabilities: SeatCapabilities,
}

impl SeatInternal for Seat {
    fn new(id: u64, capabilities: SeatCapabilities) -> Seat {
        Seat {
            id: id,
            capabilities: capabilities,
        }
    }

    fn capabilities_mut(&mut self) -> &mut SeatCapabilities {
        &mut self.capabilities
    }
}

impl Seat {
    /// Get the currently capabilities of this `Seat`
    pub fn capabilities(&self) -> &SeatCapabilities {
        &self.capabilities
    }
}

impl ::std::cmp::PartialEq for Seat {
    fn eq(&self, other: &Seat) -> bool {
        self.id == other.id
    }
}

impl ::std::hash::Hash for Seat {
    fn hash<H>(&self, state: &mut H) where H: ::std::hash::Hasher {
        self.id.hash(state);
    }
}

/// Describes capabilities a `Seat` has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SeatCapabilities {
    /// `Seat` has a pointer
    pub pointer: bool,
    /// `Seat` has a keyboard
    pub keyboard: bool,
    /// `Seat` has a touchscreen
    pub touch: bool,
}

// FIXME: Maybe refactor this into a struct or move to a more appropriate
// module once fleshed out

/// Describes a general output that can be focused by a `Seat`.
pub trait Output {
    /// Returns size in pixels (width, height)
    fn size(&self) -> (u32, u32);

    /// Returns width in pixels
    fn width(&self) -> u32;
    /// Returns height in pixels
    fn height(&self) -> u32;
}

/// State of key on a keyboard. Either pressed or released
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum KeyState {
    /// Key is released
    Released,
    /// Key is pressed
    Pressed,
}

/// A particular mouse button
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MouseButton {
    /// Left mouse button
    Left,
    /// Middle mouse button
    Middle,
    /// Right mouse button
    Right,
    /// Other mouse button with index
    Other(u8),
}

/// State of a button on a mouse. Either pressed or released
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MouseButtonState {
    /// Button is released
    Released,
    /// Button is pressed
    Pressed,
}

/// Axis when scrolling
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Axis {
    /// Vertical axis
    Vertical,
    /// Horizonal axis
    Horizontal,
}

/// Source of an axis when scrolling
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum AxisSource {
    /// Finger. Mostly used for trackpads.
    ///
    /// Guarantees that a scroll sequence is terminated with a scroll value of 0.
    /// A caller may use this information to decide on whether kinetic scrolling should
    /// be triggered on this scroll sequence.
    ///
    /// The coordinate system is identical to the
    /// cursor movement, i.e. a scroll value of 1 represents the equivalent relative
    /// motion of 1.
    Finger,
    /// Continous scrolling device. Almost identical to `Finger`
    ///
    /// No terminating event is guaranteed (though it may happen).
    ///
    /// The coordinate system is identical to
    /// the cursor movement, i.e. a scroll value of 1 represents the equivalent relative
    /// motion of 1.
    Continuous,
    /// Scroll wheel.
    ///
    /// No terminating event is guaranteed (though it may happen). Scrolling is in
    /// discrete steps. It is up to the caller how to interpret such different step sizes.
    Wheel,
    /// Scrolling through tilting the scroll wheel.
    ///
    /// No terminating event is guaranteed (though it may happen). Scrolling is in
    /// discrete steps. It is up to the caller how to interpret such different step sizes.
    WheelTilt,
}

/// Slot of a different touch event.
///
/// Touch events are groubed by slots, usually to identify different
/// fingers on a multi-touch enabled input device. Events should only
/// be interpreted in the context of other events on the same slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TouchSlot {
    id: u32,
}

impl TouchSlotInternal for TouchSlot {
    fn new(id: u32) -> Self {
        TouchSlot { id: id }
    }
}

/// Touch event
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TouchEvent {
    /// The start of an event at a given position (x, y).
    ///
    /// If the device has multi-touch capabilities a slot is given.
    Down {
        /// `TouchSlot`, if the device has multi-touch capabilities
        slot: Option<TouchSlot>,
        /// Absolute x-coordinate of the touch position.
        x: f64,
        /// Absolute y-coordinate of the touch position.
        y: f64,
    },
    /// Movement of a touch on the device surface to a given position (x, y).
    ///
    /// If the device has multi-touch capabilities a slot is given.
    Motion {
        /// `TouchSlot`, if the device has multi-touch capabilities
        slot: Option<TouchSlot>,
        /// Absolute x-coordinate of the final touch position after the motion.
        x: f64,
        /// Absolute y-coordinate of the final touch position after the motion.
        y: f64,
    },
    /// Stop of an event chain.
    ///
    /// If the device has multi-touch capabilities a slot is given.
    Up {
        /// `TouchSlot`, if the device has multi-touch capabilities
        slot: Option<TouchSlot>,
    },
    /// Cancel of an event chain. All previous events in the chain should be ignored.
    ///
    /// If the device has multi-touch capabilities a slot is given.
    Cancel {
        /// `TouchSlot`, if the device has multi-touch capabilities
        slot: Option<TouchSlot>,
    },
    /// Signals the end of a set of touchpoints at one device sample time.
    Frame,
}

/// Trait that describes objects providing a source of input events. All input backends
/// need to implemenent this and provide the same base gurantees about the presicion of
/// given events.
pub trait InputBackend: Sized {
    /// Type of input device associated with the backend
    type InputConfig: ?Sized;

    /// Type representing errors that may be returned when processing events
    type EventError: Error;

    /// Sets a new handler for this `InputBackend`
    fn set_handler<H: InputHandler<Self> + 'static>(&mut self, handler: H);
    /// Get a reference to the currently set handler, if any
    fn get_handler(&mut self) -> Option<&mut InputHandler<Self>>;
    /// Clears the currently handler, if one is set
    fn clear_handler(&mut self);

    /// Get current `InputConfig`
    fn input_config(&mut self) -> &mut Self::InputConfig;

    /// Called to inform the Input backend about a new focused Output for a `Seat`
    fn set_output_metadata(&mut self, seat: &Seat, output: &Output);

    /// Processes new events of the underlying backend and drives the `InputHandler`.
    fn dispatch_new_events(&mut self) -> Result<(), Self::EventError>;
}

/// Implement to receive input events from any `InputBackend`.
pub trait InputHandler<B: InputBackend> {
    /// Called when a new `Seat` has been created
    fn on_seat_created(&mut self, seat: &Seat);
    /// Called when an existing `Seat` has been destroyed.
    fn on_seat_destroyed(&mut self, seat: &Seat);
    /// Called when a `Seat`'s properties have changed.
    ///
    /// ## Note:
    ///
    /// It is not guaranteed that any change has actually happened.
    fn on_seat_changed(&mut self, seat: &Seat);
    /// Called when a new keyboard event was received.
    ///
    /// # Arguments
    ///
    /// - `seat` - The `Seat` the event belongs to
    /// - `time` - A upward counting variable useful for event ordering. Makes no gurantees about actual time passed between events.
    /// - `key_code` - Code of the pressed key. See linux/input-event-codes.h
    /// - `state` - `KeyState` of the event
    /// - `count` - Total number of keys pressed on all devices on the associated `Seat`
    ///
    /// # TODO:
    /// - check if events can arrive out of order.
    /// - Make stronger time guarantees
    fn on_keyboard_key(&mut self, seat: &Seat, time: u32, key_code: u32, state: KeyState, count: u32);
    /// Called when a new pointer movement event was received.
    ///
    /// # Arguments
    ///
    /// - `seat` - The `Seat` the event belongs to
    /// - `time` - A upward counting variable useful for event ordering. Makes no gurantees about actual time passed between events.
    /// - `to` - Absolute screen coordinates of the pointer moved to.
    ///
    /// # TODO:
    /// - check if events can arrive out of order.
    /// - Make stronger time guarantees
    fn on_pointer_move(&mut self, seat: &Seat, time: u32, to: (u32, u32));
    /// Called when a new pointer button event was received.
    ///
    /// # Arguments
    ///
    /// - `seat` - The `Seat` the event belongs to
    /// - `time` - A upward counting variable useful for event ordering. Makes no gurantees about actual time passed between events.
    /// - `button` - Which button was pressed..
    /// - `state` - `MouseButtonState` of the event
    ///
    /// # TODO:
    /// - check if events can arrive out of order.
    /// - Make stronger time guarantees
    fn on_pointer_button(&mut self, seat: &Seat, time: u32, button: MouseButton, state: MouseButtonState);
    /// Called when a new pointer scroll event was received.
    ///
    /// # Arguments
    ///
    /// - `seat` - The `Seat` the event belongs to
    /// - `time` - A upward counting variable useful for event ordering. Makes no gurantees about actual time passed between events.
    /// - `axis` - `Axis` this event was generated for.
    /// - `source` - Source of the scroll event. Important for interpretation of `amount`.
    /// - `amount` - Amount of scrolling on the given `Axis`. See `source` for interpretation.
    ///
    /// # TODO:
    /// - check if events can arrive out of order.
    /// - Make stronger time guarantees
    fn on_pointer_scroll(&mut self, seat: &Seat, time: u32, axis: Axis, source: AxisSource, amount: f64);
    /// Called when a new touch event was received.
    ///
    /// # Arguments
    ///
    /// - `seat` - The `Seat` the event belongs to
    /// - `time` - A upward counting variable useful for event ordering. Makes no gurantees about actual time passed between events.
    /// - `event` - Touch event recieved. See `TouchEvent`.
    ///
    /// # TODO:
    /// - check if events can arrive out of order.
    /// - Make stronger time guarantees
    fn on_touch(&mut self, seat: &Seat, time: u32, event: TouchEvent);

    /// Called when the `InputConfig` was changed through an external event.
    ///
    /// What kind of events can trigger this call is completely backend dependent.
    /// E.g. an input devices was attached/detached or changed it's own configuration.
    fn on_input_config_changed(&mut self, config: &mut B::InputConfig);
}

impl<B: InputBackend> InputHandler<B> for Box<InputHandler<B>> {
    fn on_seat_created(&mut self, seat: &Seat) {
        (**self).on_seat_created(seat)
    }

    fn on_seat_destroyed(&mut self, seat: &Seat) {
        (**self).on_seat_destroyed(seat)
    }

    fn on_seat_changed(&mut self, seat: &Seat) {
        (**self).on_seat_changed(seat)
    }

    fn on_keyboard_key(&mut self, seat: &Seat, time: u32, key_code: u32, state: KeyState, count: u32) {
        (**self).on_keyboard_key(seat, time, key_code, state, count)
    }

    fn on_pointer_move(&mut self, seat: &Seat, time: u32, to: (u32, u32)) {
        (**self).on_pointer_move(seat, time, to)
    }

    fn on_pointer_button(&mut self, seat: &Seat, time: u32, button: MouseButton, state: MouseButtonState) {
        (**self).on_pointer_button(seat, time, button, state)
    }

    fn on_pointer_scroll(&mut self, seat: &Seat, time: u32, axis: Axis, source: AxisSource, amount: f64) {
        (**self).on_pointer_scroll(seat, time, axis, source, amount)
    }

    fn on_touch(&mut self, seat: &Seat, time: u32, event: TouchEvent) {
        (**self).on_touch(seat, time, event)
    }

    fn on_input_config_changed(&mut self, config: &mut B::InputConfig) {
        (**self).on_input_config_changed(config)
    }
}
