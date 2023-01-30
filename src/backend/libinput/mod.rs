// TODO: Update to use new `Scroll*` events instead of now deprecated
// `PointerAxis*`.
#![cfg_attr(feature = "libinput_1_19", allow(deprecated))]

//! Implementation of input backend trait for types provided by `libinput`

use crate::backend::input::{self as backend, Axis, InputBackend, InputEvent};
#[cfg(feature = "backend_session")]
use crate::backend::session::{AsErrno, Session};
use input as libinput;
use input::event;

use std::{
    io,
    os::unix::io::{AsRawFd, RawFd},
    path::PathBuf,
};
#[cfg(feature = "backend_session")]
use std::{
    os::unix::io::{FromRawFd, IntoRawFd, OwnedFd},
    path::Path,
};

use calloop::{EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory};

use slog::{info, o, trace};

mod tablet;

/// Libinput based [`InputBackend`].
///
/// Tracks input of all devices given manually or via a udev seat to a provided libinput
/// context.
#[derive(Debug)]
pub struct LibinputInputBackend {
    context: libinput::Libinput,
    logger: ::slog::Logger,
    token: Option<Token>,
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
            logger: log,
            token: None,
        }
    }

    /// Returns a reference to the underlying libinput context
    pub fn context(&self) -> &libinput::Libinput {
        &self.context
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
    fn time(&self) -> u64 {
        event::keyboard::KeyboardEventTrait::time_usec(self)
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

impl backend::Event<LibinputInputBackend> for event::pointer::PointerAxisEvent {
    fn time(&self) -> u64 {
        event::pointer::PointerEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::PointerAxisEvent<LibinputInputBackend> for event::pointer::PointerAxisEvent {
    fn amount(&self, axis: Axis) -> Option<f64> {
        let axis = axis.into();
        if self.has_axis(axis) {
            Some(self.axis_value(axis))
        } else {
            None
        }
    }

    fn amount_discrete(&self, axis: Axis) -> Option<f64> {
        let axis = axis.into();
        if self.has_axis(axis) {
            self.axis_value_discrete(axis)
        } else {
            None
        }
    }

    fn source(&self) -> backend::AxisSource {
        self.axis_source().into()
    }
}

impl backend::Event<LibinputInputBackend> for event::pointer::PointerButtonEvent {
    fn time(&self) -> u64 {
        event::pointer::PointerEventTrait::time_usec(self)
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
    fn time(&self) -> u64 {
        event::pointer::PointerEventTrait::time_usec(self)
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

    fn delta_x_unaccel(&self) -> f64 {
        self.dx_unaccelerated()
    }

    fn delta_y_unaccel(&self) -> f64 {
        self.dy_unaccelerated()
    }
}

impl backend::Event<LibinputInputBackend> for event::pointer::PointerMotionAbsoluteEvent {
    fn time(&self) -> u64 {
        event::pointer::PointerEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::PointerMotionAbsoluteEvent<LibinputInputBackend>
    for event::pointer::PointerMotionAbsoluteEvent
{
}

impl backend::AbsolutePositionEvent<LibinputInputBackend> for event::pointer::PointerMotionAbsoluteEvent {
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

impl<T> backend::GestureBeginEvent<LibinputInputBackend> for T
where
    T: event::gesture::GestureEventTrait + backend::Event<LibinputInputBackend>,
{
    fn fingers(&self) -> u32 {
        self.finger_count() as u32
    }
}

impl<T> backend::GestureEndEvent<LibinputInputBackend> for T
where
    T: event::gesture::GestureEndEvent + backend::Event<LibinputInputBackend>,
{
    fn cancelled(&self) -> bool {
        self.cancelled()
    }
}

impl backend::Event<LibinputInputBackend> for event::gesture::GestureSwipeBeginEvent {
    fn time(&self) -> u64 {
        event::gesture::GestureEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::GestureSwipeBeginEvent<LibinputInputBackend> for event::gesture::GestureSwipeBeginEvent {}

impl backend::Event<LibinputInputBackend> for event::gesture::GestureSwipeUpdateEvent {
    fn time(&self) -> u64 {
        event::gesture::GestureEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::GestureSwipeUpdateEvent<LibinputInputBackend> for event::gesture::GestureSwipeUpdateEvent {
    fn delta_x(&self) -> f64 {
        event::gesture::GestureEventCoordinates::dx(self)
    }

    fn delta_y(&self) -> f64 {
        event::gesture::GestureEventCoordinates::dy(self)
    }
}

impl backend::Event<LibinputInputBackend> for event::gesture::GestureSwipeEndEvent {
    fn time(&self) -> u64 {
        event::gesture::GestureEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::GestureSwipeEndEvent<LibinputInputBackend> for event::gesture::GestureSwipeEndEvent {}

impl backend::Event<LibinputInputBackend> for event::gesture::GesturePinchBeginEvent {
    fn time(&self) -> u64 {
        event::gesture::GestureEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::GesturePinchBeginEvent<LibinputInputBackend> for event::gesture::GesturePinchBeginEvent {}

impl backend::Event<LibinputInputBackend> for event::gesture::GesturePinchUpdateEvent {
    fn time(&self) -> u64 {
        event::gesture::GestureEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::GesturePinchUpdateEvent<LibinputInputBackend> for event::gesture::GesturePinchUpdateEvent {
    fn delta_x(&self) -> f64 {
        event::gesture::GestureEventCoordinates::dx(self)
    }

    fn delta_y(&self) -> f64 {
        event::gesture::GestureEventCoordinates::dy(self)
    }

    fn scale(&self) -> f64 {
        event::gesture::GesturePinchEventTrait::scale(self)
    }

    fn rotation(&self) -> f64 {
        self.angle_delta()
    }
}

impl backend::Event<LibinputInputBackend> for event::gesture::GesturePinchEndEvent {
    fn time(&self) -> u64 {
        event::gesture::GestureEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::GesturePinchEndEvent<LibinputInputBackend> for event::gesture::GesturePinchEndEvent {}

#[cfg(feature = "libinput_1_19")]
impl backend::Event<LibinputInputBackend> for event::gesture::GestureHoldBeginEvent {
    fn time(&self) -> u64 {
        event::gesture::GestureEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

#[cfg(feature = "libinput_1_19")]
impl backend::GestureHoldBeginEvent<LibinputInputBackend> for event::gesture::GestureHoldBeginEvent {}

#[cfg(feature = "libinput_1_19")]
impl backend::Event<LibinputInputBackend> for event::gesture::GestureHoldEndEvent {
    fn time(&self) -> u64 {
        event::gesture::GestureEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

#[cfg(feature = "libinput_1_19")]
impl backend::GestureHoldEndEvent<LibinputInputBackend> for event::gesture::GestureHoldEndEvent {}

impl backend::Event<LibinputInputBackend> for event::touch::TouchDownEvent {
    fn time(&self) -> u64 {
        event::touch::TouchEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::TouchDownEvent<LibinputInputBackend> for event::touch::TouchDownEvent {}

impl backend::TouchEvent<LibinputInputBackend> for event::touch::TouchDownEvent {
    fn slot(&self) -> backend::TouchSlot {
        event::touch::TouchEventSlot::slot(self).into()
    }
}

impl backend::AbsolutePositionEvent<LibinputInputBackend> for event::touch::TouchDownEvent {
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
    fn time(&self) -> u64 {
        event::touch::TouchEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::TouchMotionEvent<LibinputInputBackend> for event::touch::TouchMotionEvent {}

impl backend::TouchEvent<LibinputInputBackend> for event::touch::TouchMotionEvent {
    fn slot(&self) -> backend::TouchSlot {
        event::touch::TouchEventSlot::slot(self).into()
    }
}

impl backend::AbsolutePositionEvent<LibinputInputBackend> for event::touch::TouchMotionEvent {
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
    fn time(&self) -> u64 {
        event::touch::TouchEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::TouchUpEvent<LibinputInputBackend> for event::touch::TouchUpEvent {}

impl backend::TouchEvent<LibinputInputBackend> for event::touch::TouchUpEvent {
    fn slot(&self) -> backend::TouchSlot {
        event::touch::TouchEventSlot::slot(self).into()
    }
}

impl backend::Event<LibinputInputBackend> for event::touch::TouchCancelEvent {
    fn time(&self) -> u64 {
        event::touch::TouchEventTrait::time_usec(self)
    }

    fn device(&self) -> libinput::Device {
        event::EventTrait::device(self)
    }
}

impl backend::TouchCancelEvent<LibinputInputBackend> for event::touch::TouchCancelEvent {}

impl backend::TouchEvent<LibinputInputBackend> for event::touch::TouchCancelEvent {
    fn slot(&self) -> backend::TouchSlot {
        event::touch::TouchEventSlot::slot(self).into()
    }
}

impl backend::Event<LibinputInputBackend> for event::touch::TouchFrameEvent {
    fn time(&self) -> u64 {
        event::touch::TouchEventTrait::time_usec(self)
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

    type GestureSwipeBeginEvent = event::gesture::GestureSwipeBeginEvent;
    type GestureSwipeUpdateEvent = event::gesture::GestureSwipeUpdateEvent;
    type GestureSwipeEndEvent = event::gesture::GestureSwipeEndEvent;
    type GesturePinchBeginEvent = event::gesture::GesturePinchBeginEvent;
    type GesturePinchUpdateEvent = event::gesture::GesturePinchUpdateEvent;
    type GesturePinchEndEvent = event::gesture::GesturePinchEndEvent;
    #[cfg(not(feature = "libinput_1_19"))]
    type GestureHoldBeginEvent = backend::UnusedEvent;
    #[cfg(not(feature = "libinput_1_19"))]
    type GestureHoldEndEvent = backend::UnusedEvent;
    #[cfg(feature = "libinput_1_19")]
    type GestureHoldBeginEvent = event::gesture::GestureHoldBeginEvent;
    #[cfg(feature = "libinput_1_19")]
    type GestureHoldEndEvent = event::gesture::GestureHoldEndEvent;

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
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        use nix::fcntl::OFlag;
        let fd = self
            .0
            .open(path, OFlag::from_bits_truncate(flags))
            .map_err(|err| err.as_errno().unwrap_or(1 /*Use EPERM by default*/))?;
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        let _ = self.0.close(fd.into_raw_fd());
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
    type Error = io::Error;

    fn process_events<F>(&mut self, _: Readiness, token: Token, mut callback: F) -> io::Result<PostAction>
    where
        F: FnMut(Self::Event, &mut ()) -> Self::Ret,
    {
        if Some(token) == self.token {
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
                    libinput::Event::Gesture(gesture_event) => match gesture_event {
                        event::GestureEvent::Swipe(event::gesture::GestureSwipeEvent::Begin(event)) => {
                            callback(InputEvent::GestureSwipeBegin { event }, &mut ());
                        }
                        event::GestureEvent::Swipe(event::gesture::GestureSwipeEvent::Update(event)) => {
                            callback(InputEvent::GestureSwipeUpdate { event }, &mut ());
                        }
                        event::GestureEvent::Swipe(event::gesture::GestureSwipeEvent::End(event)) => {
                            callback(InputEvent::GestureSwipeEnd { event }, &mut ());
                        }
                        event::GestureEvent::Pinch(event::gesture::GesturePinchEvent::Begin(event)) => {
                            callback(InputEvent::GesturePinchBegin { event }, &mut ());
                        }
                        event::GestureEvent::Pinch(event::gesture::GesturePinchEvent::Update(event)) => {
                            callback(InputEvent::GesturePinchUpdate { event }, &mut ());
                        }
                        event::GestureEvent::Pinch(event::gesture::GesturePinchEvent::End(event)) => {
                            callback(InputEvent::GesturePinchEnd { event }, &mut ());
                        }
                        #[cfg(feature = "libinput_1_19")]
                        event::GestureEvent::Hold(event::gesture::GestureHoldEvent::Begin(event)) => {
                            callback(InputEvent::GestureHoldBegin { event }, &mut ());
                        }
                        #[cfg(feature = "libinput_1_19")]
                        event::GestureEvent::Hold(event::gesture::GestureHoldEvent::End(event)) => {
                            callback(InputEvent::GestureHoldEnd { event }, &mut ());
                        }
                        _ => {
                            trace!(self.logger, "Unknown libinput gesture event");
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

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        self.token = Some(factory.token());
        poll.register(self.as_raw_fd(), Interest::READ, Mode::Level, self.token.unwrap())
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        self.token = Some(factory.token());
        poll.reregister(self.as_raw_fd(), Interest::READ, Mode::Level, self.token.unwrap())
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.token = None;
        poll.unregister(self.as_raw_fd())
    }
}
