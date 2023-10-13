//! Input backend for `libei` sender contexts
//!
//! TODO add code example

// TODO: Add helper for receiver contexts

use calloop::{EventSource, PostAction, Readiness, Token, TokenFactory};
use reis::{
    calloop::EisRequestSourceEvent,
    eis,
    request::{DeviceCapability, EisRequest},
};
use std::{
    io,
    sync::{Arc, Mutex},
};

use crate::backend::input::InputEvent;

mod input;
pub use input::ScrollEvent;
mod seat;
pub use seat::EiInputSeat;

/// An [`EventSource`] for receiving input from an EI sender context and
/// converting to [`InputEvent`]s.
#[derive(Debug)]
pub struct EiInput {
    source: reis::calloop::EisRequestSource,
    connection: Option<EiInputConnection>,
    event_sender: calloop::channel::Sender<InputEvent<EiInput>>,
    channel_source: calloop::channel::Channel<InputEvent<EiInput>>,
}

impl EiInput {
    /// Create an EI sender event source.
    ///
    /// `context` should be a new EI socket that has not been used yet.
    pub fn new(context: eis::Context) -> Self {
        let (event_sender, channel_source) = calloop::channel::channel();
        Self {
            source: reis::calloop::EisRequestSource::new(context, 0),
            event_sender,
            channel_source,
            connection: None,
        }
    }
}

/// A connection for an EI sender context that can be used to add seats and
/// devices.
#[derive(Debug)]
pub struct EiInputConnection(Arc<EiInputConnectionInner>);

#[derive(Debug)]
struct EiInputConnectionInner {
    connection: reis::request::Connection,
    event_sender: calloop::channel::Sender<InputEvent<EiInput>>,
    seats: Mutex<Vec<EiInputSeat>>,
}

impl EiInputConnection {
    fn new(
        connection: reis::request::Connection,
        event_sender: calloop::channel::Sender<InputEvent<EiInput>>,
    ) -> Self {
        Self(Arc::new(EiInputConnectionInner {
            connection,
            event_sender,
            seats: Mutex::new(Vec::new()),
        }))
    }

    /// Add a seat to the EI connection
    pub fn add_seat(&self, name: &str) -> EiInputSeat {
        let seat = self.0.connection.add_seat(
            Some(name),
            // Capabilities can't be added to a seat; so advertise them
            // all, but only create relevant devices on bind.
            DeviceCapability::Pointer
                | DeviceCapability::PointerAbsolute
                | DeviceCapability::Keyboard
                | DeviceCapability::Touch
                | DeviceCapability::Scroll
                | DeviceCapability::Button,
        );
        let seat = EiInputSeat::new(self, seat, self.0.event_sender.clone());
        self.0.seats.lock().unwrap().push(seat.clone());
        seat
    }

    /// Send buffered events on EI socket
    pub fn flush(&self) -> rustix::io::Result<()> {
        self.0.connection.flush()
    }
}

/// An event produced by an [`EiInput`] event source.
#[derive(Debug)]
pub enum EiInputEvent {
    /// The client has finished the EI handshake. Seats and devices can
    /// then be added.
    Connected,
    /// The client has disconnected from the server.
    Disconnected,
    /// An input event has been received from the client.
    Event(InputEvent<EiInput>),
}

impl EventSource for EiInput {
    type Event = EiInputEvent;
    type Metadata = EiInputConnection;
    type Ret = ();
    type Error = io::Error;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut cb: F,
    ) -> Result<PostAction, <Self as EventSource>::Error>
    where
        F: FnMut(EiInputEvent, &mut EiInputConnection),
    {
        let _ = self.channel_source.process_events(readiness, token, |event, ()| {
            if let calloop::channel::Event::Msg(event) = event {
                // Can't create device until there's a connection, so no channel messages
                let connection = self.connection.as_mut().unwrap();
                cb(EiInputEvent::Event(event), connection);
            }
        });
        self.source.process_events(readiness, token, |event, connection| {
            // Wrap connection in `EiInputConnection` if not created yet
            if self.connection.is_none() {
                self.connection = Some(EiInputConnection::new(
                    connection.clone(),
                    self.event_sender.clone(),
                ));
            }
            let connection = self.connection.as_mut().unwrap();

            match event {
                Ok(EisRequestSourceEvent::Connected) => {
                    if connection.0.connection.context_type() == eis::handshake::ContextType::Receiver {
                        connection
                            .0
                            .connection
                            .disconnected(eis::connection::DisconnectReason::Disconnected, None);
                        let _ = connection.flush();
                        return Ok(PostAction::Remove);
                    }
                    cb(EiInputEvent::Connected, connection);
                }
                Ok(EisRequestSourceEvent::Request(EisRequest::Disconnect)) => {
                    cb(EiInputEvent::Disconnected, connection);
                    return Ok(PostAction::Remove);
                }
                Ok(EisRequestSourceEvent::Request(EisRequest::Bind(request))) => {
                    if let Some(seat) = connection
                        .0
                        .seats
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|seat| **seat == request.seat)
                    {
                        seat.bind(request.capabilities);
                    }
                }
                Ok(EisRequestSourceEvent::Request(request)) => {
                    if let Some(input_event) = convert_request(request) {
                        cb(EiInputEvent::Event(input_event), connection);
                    }
                }
                Err(err) => {
                    tracing::error!("Libei client error: {}", err);
                    return Ok(PostAction::Remove);
                }
            }
            let _ = connection.flush();
            Ok(PostAction::Continue)
        })
    }

    fn register(
        &mut self,
        poll: &mut calloop::Poll,
        token_factory: &mut TokenFactory,
    ) -> Result<(), calloop::Error> {
        self.channel_source.register(poll, token_factory)?;
        self.source.register(poll, token_factory)
    }

    fn reregister(
        &mut self,
        poll: &mut calloop::Poll,
        token_factory: &mut TokenFactory,
    ) -> Result<(), calloop::Error> {
        self.channel_source.reregister(poll, token_factory)?;
        self.source.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut calloop::Poll) -> Result<(), calloop::Error> {
        self.channel_source.unregister(poll)?;
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
        EisRequest::TouchCancel(event) => Some(InputEvent::TouchCancel { event }),
        EisRequest::Frame(_) => None,
        EisRequest::Disconnect
        | EisRequest::Bind(_)
        | EisRequest::DeviceStartEmulating(_)
        | EisRequest::DeviceStopEmulating(_) => None,
    }
}
