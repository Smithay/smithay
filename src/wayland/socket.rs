//! Wayland listening socket.
//!
//! This module provides an [`EventSource`] that invokes a callback when a new client has connected to the
//! socket. This is one of the ways a Wayland compositor allows clients to discover the compositor.
//!
//! The callback provides a [`UnixStream`] that represents the client connection. You need to create the
//! client using this stream by calling [`Display::insert_client`](wayland_server::Display::insert_client).
//!
//! # Example usage
//!
//! ```no_run
//! # use std::sync::Arc;
//! use smithay::wayland::socket::ListeningSocketSource;
//!
//! // data passed into calloop
//! struct Example {
//!     display: wayland_server::Display<()>,
//! }
//!
//! let event_loop = calloop::EventLoop::<Example>::try_new().unwrap();
//! let mut display = wayland_server::Display::<()>::new().unwrap();
//!
//! // Create a socket for clients to discover the compositor.
//! //
//! // This function will select the next open name for a listening socket.
//! let listening_socket = ListeningSocketSource::new_auto(None).unwrap();
//!
//! event_loop.handle().insert_source(listening_socket, |client_stream, _, state| {
//!     // Inside the callback, you should insert the client into the display.
//!     //
//!     // You may also associate some data with the client when inserting the client.
//!     state.display.handle().insert_client(client_stream, Arc::new(ExampleClientData));
//! });
//!
//! # struct ExampleClientData;
//! #
//! # impl wayland_server::backend::ClientData for ExampleClientData {
//! #     fn initialized(&self, _: wayland_server::backend::ClientId) {}
//! #     fn disconnected(
//! #         &self,
//! #         _: wayland_server::backend::ClientId,
//! #         _: wayland_server::backend::DisconnectReason
//! #     ) {}
//! # }
//! ```

use std::{ffi::OsStr, io, os::unix::net::UnixStream};

use calloop::{
    generic::Generic, EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory,
};
use wayland_server::socket::{BindError, ListeningSocket};

/// A Wayland listening socket event source.
///
/// This implements [`EventSource`] and may be inserted into an event loop.
#[derive(Debug)]
pub struct ListeningSocketSource {
    logger: slog::Logger,
    socket: Generic<ListeningSocket>,
}

impl ListeningSocketSource {
    /// Creates a new listening socket, automatically choosing the next available `wayland` socket name.
    pub fn new_auto<L>(logger: L) -> Result<ListeningSocketSource, BindError>
    where
        L: Into<Option<::slog::Logger>>,
    {
        // Try socket numbers 1-32. Remember the upper bound of Range is exclusive.
        //
        // We don't try wayland-0 due since clients may connect to the wrong compositor. Clients these days
        // should be connecting based off the WAYLAND_DISPLAY or WAYLAND_SOCKET environment variables.
        let socket = ListeningSocket::bind_auto("wayland", 1..33)?;

        let logger = crate::slog_or_fallback(logger)
            .new(slog::o!("wayland_socket" => format!("{}", socket.socket_name().to_string_lossy())));
        slog::info!(logger, "Created new socket");

        Ok(ListeningSocketSource {
            logger,
            socket: Generic::new(socket, Interest::READ, Mode::Level),
        })
    }

    /// Creates a new listening socket with the specified name.
    pub fn with_name<L>(name: &str, logger: L) -> Result<ListeningSocketSource, BindError>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let socket = ListeningSocket::bind(name)?;
        let logger = crate::slog_or_fallback(logger)
            .new(slog::o!("wayland_socket" => format!("{}", socket.socket_name().to_string_lossy())));
        slog::info!(logger, "Created new socket");

        Ok(ListeningSocketSource {
            logger,
            socket: Generic::new(socket, Interest::READ, Mode::Level),
        })
    }

    /// Returns the name of the listening socket.
    pub fn socket_name(&self) -> &OsStr {
        self.socket.file.socket_name()
    }
}

impl EventSource for ListeningSocketSource {
    /// A stream to the new client.
    ///
    /// You must register the  client using the stream by calling
    /// [`Display::insert_client`](wayland_server::Display::insert_client).
    type Event = UnixStream;
    type Metadata = ();
    type Ret = ();

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> io::Result<PostAction>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        self.socket.process_events(readiness, token, |_, socket| {
            while let Some(client) = socket.accept()? {
                slog::debug!(self.logger, "New client connected"; "client" => format!("{:?}", client));
                callback(client, &mut ());
            }

            Ok(PostAction::Continue)
        })
    }

    fn register(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> io::Result<()> {
        self.socket.register(poll, token_factory)
    }

    fn reregister(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> io::Result<()> {
        self.socket.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> io::Result<()> {
        self.socket.unregister(poll)
    }
}
