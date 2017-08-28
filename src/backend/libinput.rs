//! Implementation of input backend trait for types provided by `libinput`

use backend::{SeatInternal, TouchSlotInternal};
use backend::input as backend;
use backend::input::InputBackend;
use input as libinput;
use input::event;
use wayland_server::{EventLoop, EventLoopHandle};
use wayland_server::sources::{FdEventSourceHandler, FdEventSource, FdInterest, READ};

use std::collections::hash_map::{DefaultHasher, Entry, HashMap};
use std::hash::{Hash, Hasher};
use std::io::Error as IoError;
use std::rc::Rc;
use std::os::unix::io::RawFd;

/// Libinput based `InputBackend`.
///
/// Tracks input of all devices given manually or via a udev seat to a provided libinput
/// context.
pub struct LibinputInputBackend {
    context: libinput::Libinput,
    devices: Vec<libinput::Device>,
    seats: HashMap<libinput::Seat, backend::Seat>,
    handler: Option<Box<backend::InputHandler<LibinputInputBackend> + 'static>>,
    logger: ::slog::Logger,
}

impl LibinputInputBackend {
    /// Initialize a new `LibinputInputBackend` from a given already initialized libinput
    /// context.
    pub fn new<L>(context: libinput::Libinput, logger: L) -> Self
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_libinput"));
        info!(log, "Initializing a libinput backend");
        LibinputInputBackend {
            context: context,
            devices: Vec::new(),
            seats: HashMap::new(),
            handler: None,
            logger: log,
        }
    }

    pub fn register(self, event_loop: &mut EventLoop) -> Result<FdEventSource, IoError> {
        let fd = unsafe { self.context.fd() };
        let id = event_loop.add_handler(self);
        event_loop.add_fd_event_source::<LibinputInputBackend>(fd, id, READ)
    }
}

impl FdEventSourceHandler for LibinputInputBackend {
    fn ready(&mut self, _evlh: &mut EventLoopHandle, _fd: RawFd, _mask: FdInterest) {
        self.dispatch_new_events().expect("Failed to dispatch new libinput events");
    }

    fn error(&mut self, _evlh: &mut EventLoopHandle, _fd: RawFd, error: IoError) {
        error!(self.logger, "Libinput event loop errored: {:?}", error);
    }
}

impl backend::Event for event::keyboard::KeyboardKeyEvent {
    fn time(&self) -> u32 {
        event::keyboard::KeyboardEventTrait::time(self)
    }
}

impl backend::KeyboardKeyEvent for event::keyboard::KeyboardKeyEvent {
    fn key_code(&self) -> u32 {
        use input::event::keyboard::KeyboardEventTrait;
        self.key()
    }

    fn state(&self) -> backend::KeyState {
        use input::event::keyboard::KeyboardEventTrait;
        self.key_state().into()
    }

    fn count(&self) -> u32 {
        self.seat_key_count()
    }
}

/// Wrapper for libinput pointer axis events to implement `backend::input::PointerAxisEvent`
pub struct PointerAxisEvent {
    axis: event::pointer::Axis,
    event: Rc<event::pointer::PointerAxisEvent>,
}

impl<'a> backend::Event for PointerAxisEvent {
    fn time(&self) -> u32 {
        use input::event::pointer::PointerEventTrait;
        self.event.time()
    }
}

impl<'a> backend::PointerAxisEvent for PointerAxisEvent {
    fn axis(&self) -> backend::Axis {
        self.axis.into()
    }

    fn source(&self) -> backend::AxisSource {
        self.event.axis_source().into()
    }

    fn amount(&self) -> f64 {
        match self.source() {
            backend::AxisSource::Finger |
            backend::AxisSource::Continuous => self.event.axis_value(self.axis),
            backend::AxisSource::Wheel |
            backend::AxisSource::WheelTilt => self.event.axis_value_discrete(self.axis).unwrap(),
        }
    }
}

impl backend::Event for event::pointer::PointerButtonEvent {
    fn time(&self) -> u32 {
        event::pointer::PointerEventTrait::time(self)
    }
}

impl backend::PointerButtonEvent for event::pointer::PointerButtonEvent {
    fn button(&self) -> backend::MouseButton {
        match self.button() {
            0x110 => backend::MouseButton::Left,
            0x111 => backend::MouseButton::Right,
            0x112 => backend::MouseButton::Middle,
            x => backend::MouseButton::Other(x as u8),
        }
    }

    fn state(&self) -> backend::MouseButtonState {
        self.button_state().into()
    }
}

impl backend::Event for event::pointer::PointerMotionEvent {
    fn time(&self) -> u32 {
        event::pointer::PointerEventTrait::time(self)
    }
}

impl backend::PointerMotionEvent for event::pointer::PointerMotionEvent {
    fn delta_x(&self) -> u32 {
        self.dx() as u32
    }
    fn delta_y(&self) -> u32 {
        self.dy() as u32
    }
}

impl backend::Event for event::pointer::PointerMotionAbsoluteEvent {
    fn time(&self) -> u32 {
        event::pointer::PointerEventTrait::time(self)
    }
}

impl backend::PointerMotionAbsoluteEvent for event::pointer::PointerMotionAbsoluteEvent {
    fn x(&self) -> f64 {
        self.absolute_x()
    }

    fn y(&self) -> f64 {
        self.absolute_y()
    }

    fn x_transformed(&self, width: u32) -> u32 {
        self.absolute_x_transformed(width) as u32
    }

    fn y_transformed(&self, height: u32) -> u32 {
        self.absolute_y_transformed(height) as u32
    }
}

impl backend::Event for event::touch::TouchDownEvent {
    fn time(&self) -> u32 {
        event::touch::TouchEventTrait::time(self)
    }
}

impl backend::TouchDownEvent for event::touch::TouchDownEvent {
    fn slot(&self) -> Option<backend::TouchSlot> {
        event::touch::TouchEventSlot::slot(self).map(|x| backend::TouchSlot::new(x as u64))
    }

    fn x(&self) -> f64 {
        event::touch::TouchEventPosition::x(self)
    }

    fn y(&self) -> f64 {
        event::touch::TouchEventPosition::y(self)
    }

    fn x_transformed(&self, width: u32) -> u32 {
        event::touch::TouchEventPosition::x_transformed(self, width) as u32
    }

    fn y_transformed(&self, height: u32) -> u32 {
        event::touch::TouchEventPosition::y_transformed(self, height) as u32
    }
}

impl backend::Event for event::touch::TouchMotionEvent {
    fn time(&self) -> u32 {
        event::touch::TouchEventTrait::time(self)
    }
}

impl backend::TouchMotionEvent for event::touch::TouchMotionEvent {
    fn slot(&self) -> Option<backend::TouchSlot> {
        event::touch::TouchEventSlot::slot(self).map(|x| backend::TouchSlot::new(x as u64))
    }

    fn x(&self) -> f64 {
        event::touch::TouchEventPosition::x(self)
    }

    fn y(&self) -> f64 {
        event::touch::TouchEventPosition::y(self)
    }

    fn x_transformed(&self, width: u32) -> u32 {
        event::touch::TouchEventPosition::x_transformed(self, width) as u32
    }

    fn y_transformed(&self, height: u32) -> u32 {
        event::touch::TouchEventPosition::y_transformed(self, height) as u32
    }
}

impl backend::Event for event::touch::TouchUpEvent {
    fn time(&self) -> u32 {
        event::touch::TouchEventTrait::time(self)
    }
}

impl backend::TouchUpEvent for event::touch::TouchUpEvent {
    fn slot(&self) -> Option<backend::TouchSlot> {
        event::touch::TouchEventSlot::slot(self).map(|x| backend::TouchSlot::new(x as u64))
    }
}

impl backend::Event for event::touch::TouchCancelEvent {
    fn time(&self) -> u32 {
        event::touch::TouchEventTrait::time(self)
    }
}

impl backend::TouchCancelEvent for event::touch::TouchCancelEvent {
    fn slot(&self) -> Option<backend::TouchSlot> {
        event::touch::TouchEventSlot::slot(self).map(|x| backend::TouchSlot::new(x as u64))
    }
}

impl backend::Event for event::touch::TouchFrameEvent {
    fn time(&self) -> u32 {
        event::touch::TouchEventTrait::time(self)
    }
}

impl backend::TouchFrameEvent for event::touch::TouchFrameEvent {}

impl backend::InputBackend for LibinputInputBackend {
    type InputConfig = [libinput::Device];
    type EventError = IoError;

    type KeyboardKeyEvent = event::keyboard::KeyboardKeyEvent;
    type PointerAxisEvent = PointerAxisEvent;
    type PointerButtonEvent = event::pointer::PointerButtonEvent;
    type PointerMotionEvent = event::pointer::PointerMotionEvent;
    type PointerMotionAbsoluteEvent = event::pointer::PointerMotionAbsoluteEvent;
    type TouchDownEvent = event::touch::TouchDownEvent;
    type TouchUpEvent = event::touch::TouchUpEvent;
    type TouchMotionEvent = event::touch::TouchMotionEvent;
    type TouchCancelEvent = event::touch::TouchCancelEvent;
    type TouchFrameEvent = event::touch::TouchFrameEvent;

    fn set_handler<H: backend::InputHandler<Self> + 'static>(&mut self, mut handler: H) {
        if self.handler.is_some() {
            self.clear_handler();
        }
        info!(self.logger, "New input handler set.");
        for seat in self.seats.values() {
            trace!(self.logger, "Calling on_seat_created with {:?}", seat);
            handler.on_seat_created(seat);
        }
        self.handler = Some(Box::new(handler));
    }

    fn get_handler(&mut self) -> Option<&mut backend::InputHandler<Self>> {
        self.handler.as_mut().map(|handler| {
            handler as &mut backend::InputHandler<Self>
        })
    }

    fn clear_handler(&mut self) {
        if let Some(mut handler) = self.handler.take() {
            for seat in self.seats.values() {
                trace!(self.logger, "Calling on_seat_destroyed with {:?}", seat);
                handler.on_seat_destroyed(seat);
            }
            info!(self.logger, "Removing input handler");
        }
    }

    fn input_config(&mut self) -> &mut Self::InputConfig {
        &mut self.devices
    }

    fn dispatch_new_events(&mut self) -> Result<(), IoError> {
        use input::event::EventTrait;

        self.context.dispatch()?;

        for event in &mut self.context {
            match event {
                libinput::Event::Device(device_event) => {
                    use input::event::device::*;
                    match device_event {
                        DeviceEvent::Added(device_added_event) => {
                            let added = device_added_event.device();

                            let new_caps = backend::SeatCapabilities {
                                pointer: added.has_capability(libinput::DeviceCapability::Pointer),
                                keyboard: added.has_capability(libinput::DeviceCapability::Keyboard),
                                touch: added.has_capability(libinput::DeviceCapability::Touch),
                            };

                            let device_seat = added.seat();
                            self.devices.push(added);

                            match self.seats.entry(device_seat) {
                                Entry::Occupied(mut seat_entry) => {
                                    let old_seat = seat_entry.get_mut();
                                    {
                                        let caps = old_seat.capabilities_mut();
                                        caps.pointer = new_caps.pointer || caps.pointer;
                                        caps.keyboard = new_caps.keyboard || caps.keyboard;
                                        caps.touch = new_caps.touch || caps.touch;
                                    }
                                    if let Some(ref mut handler) = self.handler {
                                        trace!(self.logger, "Calling on_seat_changed with {:?}", old_seat);
                                        handler.on_seat_changed(old_seat);
                                    }
                                }
                                Entry::Vacant(seat_entry) => {
                                    let mut hasher = DefaultHasher::default();
                                    seat_entry.key().hash(&mut hasher);
                                    let seat =
                                        seat_entry.insert(backend::Seat::new(hasher.finish(), new_caps));
                                    if let Some(ref mut handler) = self.handler {
                                        trace!(self.logger, "Calling on_seat_created with {:?}", seat);
                                        handler.on_seat_created(seat);
                                    }
                                }
                            }
                        }
                        DeviceEvent::Removed(device_removed_event) => {
                            let removed = device_removed_event.device();

                            // remove device
                            self.devices.retain(|dev| *dev == removed);

                            let device_seat = removed.seat();

                            // update capabilities, so they appear correctly on `on_seat_changed` and `on_seat_destroyed`.
                            if let Some(seat) = self.seats.get_mut(&device_seat) {
                                let caps = seat.capabilities_mut();
                                caps.pointer =
                                    self.devices
                                        .iter()
                                        .filter(|x| x.seat() == device_seat)
                                        .any(|x| x.has_capability(libinput::DeviceCapability::Pointer));
                                caps.keyboard =
                                    self.devices
                                        .iter()
                                        .filter(|x| x.seat() == device_seat)
                                        .any(|x| x.has_capability(libinput::DeviceCapability::Keyboard));
                                caps.touch =
                                    self.devices
                                        .iter()
                                        .filter(|x| x.seat() == device_seat)
                                        .any(|x| x.has_capability(libinput::DeviceCapability::Touch));
                            } else {
                                panic!("Seat changed that was never created")
                            }

                            // check if the seat has any other devices
                            if !self.devices.iter().any(|x| x.seat() == device_seat) {
                                // it has not, lets destroy it
                                if let Some(seat) = self.seats.remove(&device_seat) {
                                    if let Some(ref mut handler) = self.handler {
                                        trace!(self.logger, "Calling on_seat_destroyed with {:?}", seat);
                                        handler.on_seat_destroyed(&seat);
                                    }
                                } else {
                                    panic!("Seat destroyed that was never created");
                                }
                            // it has, notify about updates
                            } else if let Some(ref mut handler) = self.handler {
                                let seat = self.seats[&device_seat];
                                trace!(self.logger, "Calling on_seat_changed with {:?}", seat);
                                handler.on_seat_changed(&seat);
                            }
                        }
                    }
                    if let Some(ref mut handler) = self.handler {
                        handler.on_input_config_changed(&mut self.devices);
                    }
                }
                libinput::Event::Touch(touch_event) => {
                    use input::event::touch::*;
                    if let Some(ref mut handler) = self.handler {
                        let device_seat = touch_event.device().seat();
                        let seat = &self.seats.get(&device_seat).expect(
                            "Recieved touch event of non existing Seat",
                        );
                        match touch_event {
                            TouchEvent::Down(down_event) => {
                                trace!(self.logger, "Calling on_touch_down with {:?}", down_event);
                                handler.on_touch_down(seat, down_event)
                            }
                            TouchEvent::Motion(motion_event) => {
                                trace!(
                                    self.logger,
                                    "Calling on_touch_motion with {:?}",
                                    motion_event
                                );
                                handler.on_touch_motion(seat, motion_event)
                            }
                            TouchEvent::Up(up_event) => {
                                trace!(self.logger, "Calling on_touch_up with {:?}", up_event);
                                handler.on_touch_up(seat, up_event)
                            }
                            TouchEvent::Cancel(cancel_event) => {
                                trace!(
                                    self.logger,
                                    "Calling on_touch_cancel with {:?}",
                                    cancel_event
                                );
                                handler.on_touch_cancel(seat, cancel_event)
                            }
                            TouchEvent::Frame(frame_event) => {
                                trace!(self.logger, "Calling on_touch_frame with {:?}", frame_event);
                                handler.on_touch_frame(seat, frame_event)
                            }
                        }
                    }
                }
                libinput::Event::Keyboard(keyboard_event) => {
                    use input::event::keyboard::*;
                    match keyboard_event {
                        KeyboardEvent::Key(key_event) => {
                            if let Some(ref mut handler) = self.handler {
                                let device_seat = key_event.device().seat();
                                let seat = &self.seats.get(&device_seat).expect(
                                    "Recieved key event of non existing Seat",
                                );
                                trace!(self.logger, "Calling on_keyboard_key with {:?}", key_event);
                                handler.on_keyboard_key(seat, key_event);
                            }
                        }
                    }
                }
                libinput::Event::Pointer(pointer_event) => {
                    use input::event::pointer::*;
                    if let Some(ref mut handler) = self.handler {
                        let device_seat = pointer_event.device().seat();
                        let seat = &self.seats.get(&device_seat).expect(
                            "Recieved pointer event of non existing Seat",
                        );
                        match pointer_event {
                            PointerEvent::Motion(motion_event) => {
                                trace!(
                                    self.logger,
                                    "Calling on_pointer_move with {:?}",
                                    motion_event
                                );
                                handler.on_pointer_move(seat, motion_event);
                            }
                            PointerEvent::MotionAbsolute(motion_abs_event) => {
                                trace!(
                                    self.logger,
                                    "Calling on_pointer_move_absolute with {:?}",
                                    motion_abs_event
                                );
                                handler.on_pointer_move_absolute(seat, motion_abs_event);
                            }
                            PointerEvent::Axis(axis_event) => {
                                let rc_axis_event = Rc::new(axis_event);
                                if rc_axis_event.has_axis(Axis::Vertical) {
                                    trace!(
                                        self.logger,
                                        "Calling on_pointer_axis for Axis::Vertical with {:?}",
                                        *rc_axis_event
                                    );
                                    handler.on_pointer_axis(
                                        seat,
                                        self::PointerAxisEvent {
                                            axis: Axis::Vertical,
                                            event: rc_axis_event.clone(),
                                        },
                                    );
                                }
                                if rc_axis_event.has_axis(Axis::Horizontal) {
                                    trace!(
                                        self.logger,
                                        "Calling on_pointer_axis for Axis::Horizontal with {:?}",
                                        *rc_axis_event
                                    );
                                    handler.on_pointer_axis(
                                        seat,
                                        self::PointerAxisEvent {
                                            axis: Axis::Horizontal,
                                            event: rc_axis_event.clone(),
                                        },
                                    );
                                }
                            }
                            PointerEvent::Button(button_event) => {
                                trace!(
                                    self.logger,
                                    "Calling on_pointer_button with {:?}",
                                    button_event
                                );
                                handler.on_pointer_button(seat, button_event);
                            }
                        }
                    }
                }
                _ => {} //FIXME: What to do with the rest.
            }
        }
        Ok(())
    }
}

impl From<event::keyboard::KeyState> for backend::KeyState {
    fn from(libinput: event::keyboard::KeyState) -> Self {
        match libinput {
            event::keyboard::KeyState::Pressed => backend::KeyState::Pressed,
            event::keyboard::KeyState::Released => backend::KeyState::Released,
        }
    }
}

impl From<event::pointer::Axis> for backend::Axis {
    fn from(libinput: event::pointer::Axis) -> Self {
        match libinput {
            event::pointer::Axis::Vertical => backend::Axis::Vertical,
            event::pointer::Axis::Horizontal => backend::Axis::Horizontal,
        }
    }
}

impl From<event::pointer::AxisSource> for backend::AxisSource {
    fn from(libinput: event::pointer::AxisSource) -> Self {
        match libinput {
            event::pointer::AxisSource::Finger => backend::AxisSource::Finger,
            event::pointer::AxisSource::Continuous => backend::AxisSource::Continuous,
            event::pointer::AxisSource::Wheel => backend::AxisSource::Wheel,
            event::pointer::AxisSource::WheelTilt => backend::AxisSource::WheelTilt,
        }
    }
}

impl From<event::pointer::ButtonState> for backend::MouseButtonState {
    fn from(libinput: event::pointer::ButtonState) -> Self {
        match libinput {
            event::pointer::ButtonState::Pressed => backend::MouseButtonState::Pressed,
            event::pointer::ButtonState::Released => backend::MouseButtonState::Released,
        }
    }
}
