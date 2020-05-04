//! Implementation of input backend trait for types provided by `libinput`

mod helpers;
use helpers::{on_device_event, on_keyboard_event, on_pointer_event, on_touch_event};

use crate::backend::input::{self as backend, Axis, InputBackend, InputEvent};
#[cfg(feature = "backend_session")]
use crate::backend::session::{AsErrno, Session, SessionObserver};
use input as libinput;
use input::event;

#[cfg(feature = "backend_session")]
use std::path::Path;
use std::{
    collections::hash_map::HashMap,
    io::Error as IoError,
    os::unix::io::{AsRawFd, RawFd},
};

use calloop::{EventSource, Interest, Mode, Poll, Readiness, Token};

// No idea if this is the same across unix platforms
// Lets make this linux exclusive for now, once someone tries to build it for
// any BSD-like system, they can verify if this is right and make a PR to change this.
#[cfg(all(any(target_os = "linux", target_os = "android"), feature = "backend_session"))]
const INPUT_MAJOR: u32 = 13;

/// Libinput based [`InputBackend`].
///
/// Tracks input of all devices given manually or via a udev seat to a provided libinput
/// context.
pub struct LibinputInputBackend {
    context: libinput::Libinput,
    config: LibinputConfig,
    seats: HashMap<libinput::Seat, backend::Seat>,
    logger: ::slog::Logger,
}

impl LibinputInputBackend {
    /// Initialize a new [`LibinputInputBackend`] from a given already initialized
    /// [libinput context](libinput::Libinput).
    pub fn new<L>(context: libinput::Libinput, logger: L) -> Self
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_libinput"));
        info!(log, "Initializing a libinput backend");
        LibinputInputBackend {
            context,
            config: LibinputConfig { devices: Vec::new() },
            seats: HashMap::new(),
            logger: log,
        }
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

impl<'a> backend::Event for event::pointer::PointerAxisEvent {
    fn time(&self) -> u32 {
        event::pointer::PointerEventTrait::time(self)
    }
}

impl backend::PointerAxisEvent for event::pointer::PointerAxisEvent {
    fn amount(&self, axis: Axis) -> Option<f64> {
        Some(self.axis_value(axis.into()))
    }

    fn amount_discrete(&self, axis: Axis) -> Option<f64> {
        self.axis_value_discrete(axis.into())
    }

    fn source(&self) -> backend::AxisSource {
        self.axis_source().into()
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
    fn delta_x(&self) -> i32 {
        self.dx() as i32
    }
    fn delta_y(&self) -> i32 {
        self.dy() as i32
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

/// Special events generated by Libinput
pub enum LibinputEvent {
    /// A new device was plugged in
    NewDevice(libinput::Device),
    /// A device was plugged out
    RemovedDevice(libinput::Device),
}

/// Configuration handle for libinput
///
/// This type allows you to access the list of know devices to configure them
/// if relevant
pub struct LibinputConfig {
    devices: Vec<libinput::Device>,
}

impl LibinputConfig {
    /// Access the list of current devices
    pub fn devices(&mut self) -> &mut [libinput::Device] {
        &mut self.devices
    }
}

impl InputBackend for LibinputInputBackend {
    type EventError = IoError;

    type KeyboardKeyEvent = event::keyboard::KeyboardKeyEvent;
    type PointerAxisEvent = event::pointer::PointerAxisEvent;
    type PointerButtonEvent = event::pointer::PointerButtonEvent;
    type PointerMotionEvent = event::pointer::PointerMotionEvent;
    type PointerMotionAbsoluteEvent = event::pointer::PointerMotionAbsoluteEvent;
    type TouchDownEvent = event::touch::TouchDownEvent;
    type TouchUpEvent = event::touch::TouchUpEvent;
    type TouchMotionEvent = event::touch::TouchMotionEvent;
    type TouchCancelEvent = event::touch::TouchCancelEvent;
    type TouchFrameEvent = event::touch::TouchFrameEvent;

    type SpecialEvent = LibinputEvent;
    type InputConfig = LibinputConfig;

    fn seats(&self) -> Vec<backend::Seat> {
        self.seats.values().cloned().collect()
    }

    fn input_config(&mut self) -> &mut Self::InputConfig {
        &mut self.config
    }

    fn dispatch_new_events<F>(&mut self, mut callback: F) -> Result<(), IoError>
    where
        F: FnMut(InputEvent<Self>, &mut LibinputConfig),
    {
        self.context.dispatch()?;

        for event in &mut self.context {
            match event {
                libinput::Event::Device(device_event) => {
                    on_device_event(
                        &mut callback,
                        &mut self.seats,
                        &mut self.config,
                        device_event,
                        &self.logger,
                    );
                }
                libinput::Event::Touch(touch_event) => {
                    on_touch_event(
                        &mut callback,
                        &self.seats,
                        &mut self.config,
                        touch_event,
                        &self.logger,
                    );
                }
                libinput::Event::Keyboard(keyboard_event) => {
                    on_keyboard_event(
                        &mut callback,
                        &self.seats,
                        &mut self.config,
                        keyboard_event,
                        &self.logger,
                    );
                }
                libinput::Event::Pointer(pointer_event) => {
                    on_pointer_event(
                        &mut callback,
                        &self.seats,
                        &mut self.config,
                        pointer_event,
                        &self.logger,
                    );
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

impl From<backend::Axis> for event::pointer::Axis {
    fn from(axis: backend::Axis) -> Self {
        match axis {
            backend::Axis::Vertical => event::pointer::Axis::Vertical,
            backend::Axis::Horizontal => event::pointer::Axis::Horizontal,
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

#[cfg(feature = "backend_session")]
impl SessionObserver for libinput::Libinput {
    fn pause(&mut self, device: Option<(u32, u32)>) {
        if let Some((major, _)) = device {
            if major != INPUT_MAJOR {
                return;
            }
        }
        // lets hope multiple suspends are okay in case of logind?
        self.suspend()
    }

    fn activate(&mut self, _device: Option<(u32, u32, Option<RawFd>)>) {
        // libinput closes the devices on suspend, so we should not get any INPUT_MAJOR calls
        // also lets hope multiple resumes are okay in case of logind
        self.resume().expect("Unable to resume libinput context");
    }
}

/// Wrapper for types implementing the [`Session`] trait to provide
/// a [`libinput::LibinputInterface`] implementation.
#[cfg(feature = "backend_session")]
pub struct LibinputSessionInterface<S: Session>(S);

#[cfg(feature = "backend_session")]
impl<S: Session> From<S> for LibinputSessionInterface<S> {
    fn from(session: S) -> LibinputSessionInterface<S> {
        LibinputSessionInterface(session)
    }
}

#[cfg(feature = "backend_session")]
impl<S: Session> libinput::LibinputInterface for LibinputSessionInterface<S> {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<RawFd, i32> {
        use nix::fcntl::OFlag;
        self.0
            .open(path, OFlag::from_bits_truncate(flags))
            .map_err(|err| err.as_errno().unwrap_or(1 /*Use EPERM by default*/))
    }

    fn close_restricted(&mut self, fd: RawFd) {
        let _ = self.0.close(fd);
    }
}

impl AsRawFd for LibinputInputBackend {
    fn as_raw_fd(&self) -> RawFd {
        self.context.as_raw_fd()
    }
}

impl EventSource for LibinputInputBackend {
    type Event = InputEvent<LibinputInputBackend>;
    type Metadata = LibinputConfig;
    type Ret = ();

    fn process_events<F>(&mut self, _: Readiness, _: Token, callback: F) -> std::io::Result<()>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        self.dispatch_new_events(callback)
    }

    fn register(&mut self, poll: &mut Poll, token: Token) -> std::io::Result<()> {
        poll.register(self.as_raw_fd(), Interest::Readable, Mode::Level, token)
    }

    fn reregister(&mut self, poll: &mut Poll, token: Token) -> std::io::Result<()> {
        poll.reregister(self.as_raw_fd(), Interest::Readable, Mode::Level, token)
    }

    fn unregister(&mut self, poll: &mut Poll) -> std::io::Result<()> {
        poll.unregister(self.as_raw_fd())
    }
}
