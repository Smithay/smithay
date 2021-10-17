//! Helper utilities for using x11rb as an event source in calloop.
//!
//! The primary use for this module is XWayland integration but is also widely useful for an X11
//! backend in a compositor.

use std::{
    io::Result as IOResult,
    sync::Arc,
    thread::{spawn, JoinHandle},
};

use x11rb::{
    connection::Connection as _,
    protocol::{
        xproto::{Atom, ClientMessageEvent, ConnectionExt as _, EventMask, Window, CLIENT_MESSAGE_EVENT},
        Event,
    },
    rust_connection::RustConnection,
};

use calloop::{
    channel::{sync_channel, Channel, Event as ChannelEvent, SyncSender},
    EventSource, Poll, PostAction, Readiness, Token, TokenFactory,
};

/// Integration of an x11rb X11 connection with calloop.
///
/// This is a thin wrapper around `Channel`. It works by spawning an extra thread reads events from
/// the X11 connection and then sends them across the channel.
///
/// See [1] for why this extra thread is necessary. The single-thread solution proposed on that
/// page does not work with calloop, since it requires checking something on every main loop
/// iteration. Calloop only allows "when an FD becomes readable".
///
/// [1]: https://docs.rs/x11rb/0.8.1/x11rb/event_loop_integration/index.html#threads-and-races
#[derive(Debug)]
pub struct X11Source {
    connection: Arc<RustConnection>,
    channel: Option<Channel<Event>>,
    event_thread: Option<JoinHandle<()>>,
    close_window: Window,
    close_type: Atom,
    log: slog::Logger,
}

impl X11Source {
    /// Create a new X11 source.
    ///
    /// The returned instance will use `SendRequest` to cause a `ClientMessageEvent` to be sent to
    /// the given window with the given type. The expectation is that this is a window that was
    /// created by us. Thus, the event reading thread will wake up and check an internal exit flag,
    /// then exit.
    pub fn new(
        connection: Arc<RustConnection>,
        close_window: Window,
        close_type: Atom,
        log: slog::Logger,
    ) -> Self {
        let (sender, channel) = sync_channel(5);
        let conn = Arc::clone(&connection);
        let log2 = log.clone();
        let event_thread = Some(spawn(move || {
            run_event_thread(conn, sender, log2);
        }));

        Self {
            connection,
            channel: Some(channel),
            event_thread,
            close_window,
            close_type,
            log,
        }
    }
}

impl Drop for X11Source {
    fn drop(&mut self) {
        // Signal the worker thread to exit by dropping the read end of the channel.
        self.channel.take();

        // Send an event to wake up the worker so that it actually exits
        let event = ClientMessageEvent {
            response_type: CLIENT_MESSAGE_EVENT,
            format: 8,
            sequence: 0,
            window: self.close_window,
            type_: self.close_type,
            data: [0; 20].into(),
        };

        let _ = self
            .connection
            .send_event(false, self.close_window, EventMask::NO_EVENT, event);
        let _ = self.connection.flush();

        // Wait for the worker thread to exit
        self.event_thread.take().map(|handle| handle.join());
    }
}

impl EventSource for X11Source {
    type Event = Event;
    type Metadata = ();
    type Ret = ();

    fn process_events<C>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: C,
    ) -> IOResult<PostAction>
    where
        C: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let log = self.log.clone();

        if let Some(channel) = &mut self.channel {
            channel.process_events(readiness, token, move |event, meta| match event {
                ChannelEvent::Closed => slog::warn!(log, "Event thread exited"),
                ChannelEvent::Msg(event) => callback(event, meta),
            })
        } else {
            Ok(PostAction::Remove)
        }
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> IOResult<()> {
        if let Some(channel) = &mut self.channel {
            channel.register(poll, factory)?;
        }

        Ok(())
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> IOResult<()> {
        if let Some(channel) = &mut self.channel {
            channel.reregister(poll, factory)?;
        }

        Ok(())
    }

    fn unregister(&mut self, poll: &mut Poll) -> IOResult<()> {
        if let Some(channel) = &mut self.channel {
            channel.unregister(poll)?;
        }

        Ok(())
    }
}

/// This thread reads X11 events from the connection and sends them on the channel.
///
/// This is run in an extra thread since sending an X11 request or waiting for the reply to an X11
/// request can both read X11 events from the underlying socket which are then saved in the
/// RustConnection. Thus, readability of the underlying socket is not enough to guarantee we do not
/// miss wakeups.
///
/// This thread will call wait_for_event(). RustConnection then ensures internally to wake us up
/// when an event arrives. So far, this seems to be the only safe way to integrate x11rb with
/// calloop.
fn run_event_thread(connection: Arc<RustConnection>, sender: SyncSender<Event>, log: slog::Logger) {
    loop {
        let event = match connection.wait_for_event() {
            Ok(event) => event,
            Err(err) => {
                // Connection errors are most likely permanent. Thus, exit the thread.
                slog::crit!(log, "Event thread exiting due to connection error {}", err);
                break;
            }
        };
        match sender.send(event) {
            Ok(()) => {}
            Err(_) => {
                // The only possible error is that the other end of the channel was dropped.
                // This happens in X11Source's Drop impl.
                break;
            }
        }
    }
}
