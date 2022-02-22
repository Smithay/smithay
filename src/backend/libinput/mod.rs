//! Implementation of input backend trait for types provided by `libinput`

use crate::backend::input::{self as backend, Axis, InputBackend, InputEvent};
#[cfg(feature = "backend_session")]
use crate::{
    backend::session::{AsErrno, Session, Signal as SessionSignal},
    utils::signaling::{Linkable, SignalToken, Signaler},
};
use input as libinput;
use input::event;

#[cfg(feature = "backend_session")]
use std::path::Path;
use std::{
    os::unix::io::{AsRawFd, RawFd},
    path::PathBuf,
};

use calloop::{EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory};

use slog::{info, o, trace};

mod tablet;

// No idea if this is the same across unix platforms
// Lets make this linux exclusive for now, once someone tries to build it for
// any BSD-like system, they can verify if this is right and make a PR to change this.
#[cfg(all(any(target_os = "linux", target_os = "android"), feature = "backend_session"))]
const INPUT_MAJOR: u32 = 13;

/// Libinput based [`InputBackend`].
///
/// Tracks input of all devices given manually or via a udev seat to a provided libinput
/// context.
#[derive(Debug)]
pub struct LibinputInputBackend {
    context: libinput::Libinput,
    #[cfg(feature = "backend_session")]
    links: Vec<SignalToken>,
    logger: ::slog::Logger,
    token: Token,
}

impl LibinputInputBackend {
    /// Initialize a new [`LibinputInputBackend`] from a given already initialized
    /// [libinput context](libinput::Libinput).
    pub fn new<L>(context: libinput::Libinput, logger: L) -> Self
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_libinput"));
        info!(log, "Initializing a libinput backend");
        LibinputInputBackend {
            context,
            #[cfg(feature = "backend_session")]
            links: Vec::new(),
            logger: log,
            token: Token::invalid(),
        }
    }
}

#[cfg(feature = "backend_session")]
impl Linkable<SessionSignal> for LibinputInputBackend {
    fn link(&mut self, signaler: Signaler<SessionSignal>) {
        let mut input = self.context.clone();
        let log = self.logger.clone();
        let token = signaler.register(move |s| match s {
            SessionSignal::PauseSession
            | SessionSignal::PauseDevice {
                major: INPUT_MAJOR, ..
            } => {
                input.suspend();
            }
            SessionSignal::ActivateSession | SessionSignal::ActivateDevice { .. } => {
                if input.resume().is_err() {
                    slog::error!(log, "Failed to resume libinput context");
                }
            }
            _ => {}
        });
        self.links.push(token);
    }
}

impl backend::Device for libinput::Device {
    fn id(&self) -> String {
        self.sysname().into()
    }

    fn name(&self) -> String {
        self.name().into()
    }

    fn has_capability(&self, capability: backend::DeviceCapability) -> bool {
        libinput::Device::has_capability(self, capability.into())
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        Some((
            libinput::Device::id_product(self),
            libinput::Device::id_vendor(self),
        ))
    }

    fn syspath(&self) -> Option<PathBuf> {
        #[cfg(feature = "udev")]
        return unsafe { libinput::Device::udev_device(self) }.map(|d| d.syspath().to_owned());

        #[cfg(not(feature = "udev"))]
        None
    }
}

impl From<backend::DeviceCapability> for libinput::DeviceCapability {
    fn from(other: backend::DeviceCapability) -> libinput::DeviceCapability {
        match other {
            backend::DeviceCapability::Gesture => libinput::DeviceCapability::Gesture,
            backend::DeviceCapability::Keyboard => libinput::DeviceCapability::Keyboard,
            backend::DeviceCapability::Pointer => libinput::DeviceCapability::Pointer,
            backend::DeviceCapability::Switch => libinput::DeviceCapability::Switch,
            backend::DeviceCapability::TabletPad => libinput::DeviceCapability::TabletPad,
            backend::DeviceCapability::TabletTool => libinput::DeviceCapability::TabletTool,
            backend::DeviceCapability::Touch => libinput::DeviceCapability::Touch,
        }
    }
}

impl backend::Event<LibinputInputBackend> for event::keyboard::KeyboardKeyEvent {
    fn time(&self) -> u32 {
        event::keyboard::KeyboardEventTrait::time(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::KeyboardKeyEvent<LibinputInputBackend> for event::keyboard::KeyboardKeyEvent {
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

impl<'a> backend::Event<LibinputInputBackend> for event::pointer::PointerAxisEvent {
    fn time(&self) -> u32 {
        event::pointer::PointerEventTrait::time(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::PointerAxisEvent<LibinputInputBackend> for event::pointer::PointerAxisEvent {
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

impl backend::Event<LibinputInputBackend> for event::pointer::PointerButtonEvent {
    fn time(&self) -> u32 {
        event::pointer::PointerEventTrait::time(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::PointerButtonEvent<LibinputInputBackend> for event::pointer::PointerButtonEvent {
    fn button_code(&self) -> u32 {
        self.button()
    }

    fn state(&self) -> backend::ButtonState {
        self.button_state().into()
    }
}

impl backend::Event<LibinputInputBackend> for event::pointer::PointerMotionEvent {
    fn time(&self) -> u32 {
        event::pointer::PointerEventTrait::time(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::PointerMotionEvent<LibinputInputBackend> for event::pointer::PointerMotionEvent {
    fn delta_x(&self) -> f64 {
        self.dx()
    }
    fn delta_y(&self) -> f64 {
        self.dy()
    }
}

impl backend::Event<LibinputInputBackend> for event::pointer::PointerMotionAbsoluteEvent {
    fn time(&self) -> u32 {
        event::pointer::PointerEventTrait::time(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::PointerMotionAbsoluteEvent<LibinputInputBackend>
    for event::pointer::PointerMotionAbsoluteEvent
{
    fn x(&self) -> f64 {
        self.absolute_x()
    }

    fn y(&self) -> f64 {
        self.absolute_y()
    }

    fn x_transformed(&self, width: i32) -> f64 {
        self.absolute_x_transformed(width as u32)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        self.absolute_y_transformed(height as u32)
    }
}

impl backend::Event<LibinputInputBackend> for event::touch::TouchDownEvent {
    fn time(&self) -> u32 {
        event::touch::TouchEventTrait::time(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::TouchDownEvent<LibinputInputBackend> for event::touch::TouchDownEvent {
    fn slot(&self) -> backend::TouchSlot {
        event::touch::TouchEventSlot::slot(self).into()
    }

    fn x(&self) -> f64 {
        event::touch::TouchEventPosition::x(self)
    }

    fn y(&self) -> f64 {
        event::touch::TouchEventPosition::y(self)
    }

    fn x_transformed(&self, width: i32) -> f64 {
        event::touch::TouchEventPosition::x_transformed(self, width as u32)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        event::touch::TouchEventPosition::y_transformed(self, height as u32)
    }
}

impl backend::Event<LibinputInputBackend> for event::touch::TouchMotionEvent {
    fn time(&self) -> u32 {
        event::touch::TouchEventTrait::time(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::TouchMotionEvent<LibinputInputBackend> for event::touch::TouchMotionEvent {
    fn slot(&self) -> backend::TouchSlot {
        event::touch::TouchEventSlot::slot(self).into()
    }

    fn x(&self) -> f64 {
        event::touch::TouchEventPosition::x(self)
    }

    fn y(&self) -> f64 {
        event::touch::TouchEventPosition::y(self)
    }

    fn x_transformed(&self, width: i32) -> f64 {
        event::touch::TouchEventPosition::x_transformed(self, width as u32)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        event::touch::TouchEventPosition::y_transformed(self, height as u32)
    }
}

impl backend::Event<LibinputInputBackend> for event::touch::TouchUpEvent {
    fn time(&self) -> u32 {
        event::touch::TouchEventTrait::time(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::TouchUpEvent<LibinputInputBackend> for event::touch::TouchUpEvent {
    fn slot(&self) -> backend::TouchSlot {
        event::touch::TouchEventSlot::slot(self).into()
    }
}

impl backend::Event<LibinputInputBackend> for event::touch::TouchCancelEvent {
    fn time(&self) -> u32 {
        event::touch::TouchEventTrait::time(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::TouchCancelEvent<LibinputInputBackend> for event::touch::TouchCancelEvent {
    fn slot(&self) -> backend::TouchSlot {
        event::touch::TouchEventSlot::slot(self).into()
    }
}

impl backend::Event<LibinputInputBackend> for event::touch::TouchFrameEvent {
    fn time(&self) -> u32 {
        event::touch::TouchEventTrait::time(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::TouchFrameEvent<LibinputInputBackend> for event::touch::TouchFrameEvent {}

impl InputBackend for LibinputInputBackend {
    type Device = libinput::Device;
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
    type TabletToolAxisEvent = event::tablet_tool::TabletToolAxisEvent;
    type TabletToolProximityEvent = event::tablet_tool::TabletToolProximityEvent;
    type TabletToolTipEvent = event::tablet_tool::TabletToolTipEvent;
    type TabletToolButtonEvent = event::tablet_tool::TabletToolButtonEvent;

    type SpecialEvent = backend::UnusedEvent;
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

impl From<event::pointer::ButtonState> for backend::ButtonState {
    fn from(libinput: event::pointer::ButtonState) -> Self {
        match libinput {
            event::pointer::ButtonState::Pressed => backend::ButtonState::Pressed,
            event::pointer::ButtonState::Released => backend::ButtonState::Released,
        }
    }
}

/// Wrapper for types implementing the [`Session`] trait to provide
/// a [`libinput::LibinputInterface`] implementation.
#[cfg(feature = "backend_session")]
#[derive(Debug)]
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
    type Metadata = ();
    type Ret = ();

    fn process_events<F>(
        &mut self,
        _: Readiness,
        token: Token,
        mut callback: F,
    ) -> std::io::Result<PostAction>
    where
        F: FnMut(Self::Event, &mut ()) -> Self::Ret,
    {
        if token == self.token {
            self.context.dispatch()?;

            for event in &mut self.context {
                match event {
                    libinput::Event::Device(device_event) => match device_event {
                        event::DeviceEvent::Added(device_added_event) => {
                            let added = event::EventTrait::device(&device_added_event);

                            info!(self.logger, "New device {:?}", added.sysname(),);

                            callback(InputEvent::DeviceAdded { device: added }, &mut ());
                        }
                        event::DeviceEvent::Removed(device_removed_event) => {
                            let removed = event::EventTrait::device(&device_removed_event);

                            info!(self.logger, "Removed device {:?}", removed.sysname(),);

                            callback(InputEvent::DeviceRemoved { device: removed }, &mut ());
                        }
                        _ => {
                            trace!(self.logger, "Unknown libinput device event");
                        }
                    },
                    libinput::Event::Touch(touch_event) => match touch_event {
                        event::TouchEvent::Down(down_event) => {
                            callback(InputEvent::TouchDown { event: down_event }, &mut ());
                        }
                        event::TouchEvent::Motion(motion_event) => {
                            callback(InputEvent::TouchMotion { event: motion_event }, &mut ());
                        }
                        event::TouchEvent::Up(up_event) => {
                            callback(InputEvent::TouchUp { event: up_event }, &mut ());
                        }
                        event::TouchEvent::Cancel(cancel_event) => {
                            callback(InputEvent::TouchCancel { event: cancel_event }, &mut ());
                        }
                        event::TouchEvent::Frame(frame_event) => {
                            callback(InputEvent::TouchFrame { event: frame_event }, &mut ());
                        }
                        _ => {
                            trace!(self.logger, "Unknown libinput touch event");
                        }
                    },
                    libinput::Event::Keyboard(keyboard_event) => match keyboard_event {
                        event::KeyboardEvent::Key(key_event) => {
                            callback(InputEvent::Keyboard { event: key_event }, &mut ());
                        }
                        _ => {
                            trace!(self.logger, "Unknown libinput keyboard event");
                        }
                    },
                    libinput::Event::Pointer(pointer_event) => match pointer_event {
                        event::PointerEvent::Motion(motion_event) => {
                            callback(InputEvent::PointerMotion { event: motion_event }, &mut ());
                        }
                        event::PointerEvent::MotionAbsolute(motion_abs_event) => {
                            callback(
                                InputEvent::PointerMotionAbsolute {
                                    event: motion_abs_event,
                                },
                                &mut (),
                            );
                        }
                        event::PointerEvent::Axis(axis_event) => {
                            callback(InputEvent::PointerAxis { event: axis_event }, &mut ());
                        }
                        event::PointerEvent::Button(button_event) => {
                            callback(InputEvent::PointerButton { event: button_event }, &mut ());
                        }
                        _ => {
                            trace!(self.logger, "Unknown libinput pointer event");
                        }
                    },
                    libinput::Event::Tablet(tablet_event) => match tablet_event {
                        event::TabletToolEvent::Axis(event) => {
                            callback(InputEvent::TabletToolAxis { event }, &mut ());
                        }
                        event::TabletToolEvent::Proximity(event) => {
                            callback(InputEvent::TabletToolProximity { event }, &mut ());
                        }
                        event::TabletToolEvent::Tip(event) => {
                            callback(InputEvent::TabletToolTip { event }, &mut ());
                        }
                        event::TabletToolEvent::Button(event) => {
                            callback(InputEvent::TabletToolButton { event }, &mut ());
                        }
                        _ => {
                            trace!(self.logger, "Unknown libinput tablet event");
                        }
                    },
                    _ => {} //FIXME: What to do with the rest.
                }
            }
        }

        Ok(PostAction::Continue)
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> std::io::Result<()> {
        self.token = factory.token();
        poll.register(self.as_raw_fd(), Interest::READ, Mode::Level, self.token)
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> std::io::Result<()> {
        self.token = factory.token();
        poll.reregister(self.as_raw_fd(), Interest::READ, Mode::Level, self.token)
    }

    fn unregister(&mut self, poll: &mut Poll) -> std::io::Result<()> {
        self.token = Token::invalid();
        poll.unregister(self.as_raw_fd())
    }
}
