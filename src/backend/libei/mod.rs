// Have some way to set if socket can be used for reciver, sender, or both?
// - restrict what devices it can use?
// For emulation:
// - create a seat for each seat
// - create a device for pointer, touch, keyboard, if the seat has that
//   * send keymap for keyboard
// - direct emulated input on these to the relevant handles
// For reciever context:
// - do we need to pass the application any requests from the client?
// re-export listener source?

use calloop::{EventSource, PostAction, Readiness, Token, TokenFactory};
use reis::{
    calloop::EisRequestSourceEvent,
    eis::{self, device::DeviceType},
    request::{self, Connection, DeviceCapability, EisRequest},
};
use rustix::fd::AsFd;
use std::{ffi::CStr, io, path::PathBuf};
use xkbcommon::xkb;

use crate::{
    backend::input::{self, InputBackend, InputEvent},
    input::keyboard::XkbConfig,
    utils::SealedFile,
};

struct SenderState {
    name: Option<String>,
    connection: eis::Connection,
    seat: eis::Seat,
    last_serial: u32,
}

impl SenderState {
    fn new(name: Option<String>, connection: eis::Connection) -> Self {
        // TODO create seat, etc.
        // check protocol versions
        let seat = connection.seat(1);
        seat.name("default");
        seat.capability(0x2, "ei_pointer");
        seat.capability(0x4, "ei_pointer_absolute");
        seat.capability(0x8, "ei_button");
        seat.capability(0x10, "ei_scroll");
        seat.capability(0x20, "ei_keyboard");
        seat.capability(0x40, "ei_touchscreen");
        seat.done();
        Self {
            name,
            connection,
            seat,
            last_serial: 0,
        }
    }
}

#[derive(Debug)]
pub struct EiInput {
    source: reis::calloop::EisRequestSource,
    seat: Option<reis::request::Seat>,
}

impl EiInput {
    pub fn new(context: eis::Context) -> Self {
        Self {
            source: reis::calloop::EisRequestSource::new(context, 0),
            seat: None,
        }
    }
}

fn disconnected(
    connection: &Connection,
    reason: eis::connection::DisconnectReason,
    explaination: &str,
) -> io::Result<calloop::PostAction> {
    connection.disconnected(reason, explaination);
    connection.flush();
    Ok(calloop::PostAction::Remove)
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
    type TouchCancelEvent = input::UnusedEvent; // XXX?
    type TouchFrameEvent = input::UnusedEvent; // XXX

    type TabletToolAxisEvent = input::UnusedEvent;
    type TabletToolProximityEvent = input::UnusedEvent;
    type TabletToolTipEvent = input::UnusedEvent;
    type TabletToolButtonEvent = input::UnusedEvent;

    type SwitchToggleEvent = input::UnusedEvent;

    type SpecialEvent = input::UnusedEvent;
}

impl input::Device for request::Device {
    fn id(&self) -> String {
        self.name().unwrap_or("").to_string()
    }

    fn name(&self) -> String {
        self.name().unwrap_or("").to_string()
    }

    fn has_capability(&self, capability: input::DeviceCapability) -> bool {
        if let Ok(capability) = DeviceCapability::try_from(capability) {
            self.has_capability(capability)
        } else {
            false
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

pub enum ScrollEvent {
    Delta(request::ScrollDelta),
    Cancel(request::ScrollCancel),
    Discrete(request::ScrollDiscrete),
    Stop(request::ScrollStop),
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

impl EventSource for EiInput {
    type Event = InputEvent<EiInput>;
    type Metadata = ();
    type Ret = ();
    type Error = io::Error;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut cb: F,
    ) -> Result<PostAction, <Self as EventSource>::Error>
    where
        F: FnMut(InputEvent<EiInput>, &mut ()) -> (),
    {
        self.source.process_events(readiness, token, |event, connection| {
            match event {
                Ok(EisRequestSourceEvent::Connected) => {
                    let seat = connection.add_seat(
                        Some("default"),
                        &[
                            DeviceCapability::Pointer,
                            DeviceCapability::PointerAbsolute,
                            DeviceCapability::Keyboard,
                            DeviceCapability::Touch,
                            DeviceCapability::Scroll,
                            DeviceCapability::Button,
                        ],
                    );

                    self.seat = Some(seat);
                }
                Ok(EisRequestSourceEvent::Request(EisRequest::Disconnect)) => {
                    return Ok(PostAction::Remove);
                }
                Ok(EisRequestSourceEvent::Request(EisRequest::Bind(request))) => {
                    let capabilities = request.capabilities;

                    // TODO Handle in converter
                    if capabilities & 0x7e != capabilities {
                        return disconnected(
                            connection,
                            eis::connection::DisconnectReason::Value,
                            "Invalid capabilities",
                        );
                    }

                    let seat = self.seat.as_ref().unwrap();

                    if connection.has_interface("ei_keyboard")
                        && capabilities & 2 << DeviceCapability::Keyboard as u64 != 0
                    {
                        // XXX use seat keymap
                        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                        let keymap = XkbConfig::default().compile_keymap(&context).unwrap();
                        let keymap_text = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
                        let file = SealedFile::with_data(
                            CStr::from_bytes_with_nul(b"eis-keymap\0").unwrap(),
                            keymap_text.as_bytes(),
                        )
                        .unwrap();

                        let device = seat.add_device(
                            Some("keyboard"),
                            DeviceType::Virtual,
                            &[DeviceCapability::Keyboard],
                            |device| {
                                let keyboard = device.interface::<eis::Keyboard>().unwrap();
                                keyboard.keymap(
                                    eis::keyboard::KeymapType::Xkb,
                                    keymap_text.len() as _,
                                    file.as_fd(),
                                );
                            },
                        );
                    }

                    // XXX button/etc should be on same object
                    if connection.has_interface("ei_pointer")
                        && capabilities & 2 << DeviceCapability::Pointer as u64 != 0
                    {
                        seat.add_device(
                            Some("pointer"),
                            DeviceType::Virtual,
                            &[DeviceCapability::Pointer],
                            |_| {},
                        );
                    }

                    if connection.has_interface("ei_touchscreen")
                        && capabilities & 2 << DeviceCapability::Touch as u64 != 0
                    {
                        seat.add_device(
                            Some("touch"),
                            DeviceType::Virtual,
                            &[DeviceCapability::Touch],
                            |_| {},
                        );
                    }

                    if connection.has_interface("ei_pointer_absolute")
                        && capabilities & 2 << DeviceCapability::PointerAbsolute as u64 != 0
                    {
                        seat.add_device(
                            Some("pointer-abs"),
                            DeviceType::Virtual,
                            &[DeviceCapability::PointerAbsolute],
                            |_| {},
                        );
                    }

                    // TODO create devices; compare against current bitflag
                }
                Ok(EisRequestSourceEvent::Request(request)) => {
                    if let Some(input_event) = convert_request(request) {
                        cb(input_event, &mut ());
                    }
                }
                Ok(EisRequestSourceEvent::InvalidObject(_object_id)) => {}
                Err(err) => {
                    tracing::error!("Libei client error: {}", err);
                    return Ok(PostAction::Remove);
                }
            }
            connection.flush();
            Ok(PostAction::Continue)
        })
    }

    fn register(
        &mut self,
        poll: &mut calloop::Poll,
        token_factory: &mut TokenFactory,
    ) -> Result<(), calloop::Error> {
        self.source.register(poll, token_factory)
    }

    fn reregister(
        &mut self,
        poll: &mut calloop::Poll,
        token_factory: &mut TokenFactory,
    ) -> Result<(), calloop::Error> {
        self.source.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut calloop::Poll) -> Result<(), calloop::Error> {
        self.source.unregister(poll)
    }
}

fn convert_request(request: EisRequest) -> Option<InputEvent<EiInput>> {
    match request {
        EisRequest::KeyboardKey(event) => Some(InputEvent::Keyboard { event }),
        EisRequest::PointerMotion(event) => Some(InputEvent::PointerMotion { event }),
        EisRequest::PointerMotionAbsolute(event) => Some(InputEvent::PointerMotionAbsolute { event }),
        EisRequest::Button(event) => Some(InputEvent::PointerButton { event }),
        EisRequest::ScrollDelta(event) => Some(InputEvent::PointerAxis {
            event: ScrollEvent::Delta(event),
        }),
        EisRequest::ScrollStop(event) => Some(InputEvent::PointerAxis {
            event: ScrollEvent::Stop(event),
        }),
        EisRequest::ScrollCancel(event) => Some(InputEvent::PointerAxis {
            event: ScrollEvent::Cancel(event),
        }),
        EisRequest::ScrollDiscrete(event) => Some(InputEvent::PointerAxis {
            event: ScrollEvent::Discrete(event),
        }),
        EisRequest::TouchDown(event) => Some(InputEvent::TouchDown { event }),
        EisRequest::TouchUp(event) => Some(InputEvent::TouchUp { event }),
        EisRequest::TouchMotion(event) => Some(InputEvent::TouchMotion { event }),
        EisRequest::Frame(_) => None,
        EisRequest::Disconnect
        | EisRequest::Bind(_)
        | EisRequest::DeviceStartEmulating(_)
        | EisRequest::DeviceStopEmulating(_) => None,
    }
}

// XXX not a direct match?
impl TryFrom<input::DeviceCapability> for DeviceCapability {
    type Error = ();
    fn try_from(other: input::DeviceCapability) -> Result<DeviceCapability, ()> {
        match other {
            input::DeviceCapability::Gesture => Err(()),
            input::DeviceCapability::Keyboard => Ok(DeviceCapability::Keyboard),
            input::DeviceCapability::Pointer => Ok(DeviceCapability::Pointer),
            input::DeviceCapability::Switch => Err(()),
            input::DeviceCapability::TabletPad => Err(()),
            input::DeviceCapability::TabletTool => Err(()),
            input::DeviceCapability::Touch => Ok(DeviceCapability::Touch),
        }
    }
}
