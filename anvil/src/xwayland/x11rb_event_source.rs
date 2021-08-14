use std::{
    io::Result as IOResult,
    sync::{atomic::{AtomicBool, Ordering}, Arc},
    thread::{spawn, JoinHandle},
};

use x11rb::{
    connection::Connection as _, protocol::{Event, xproto::{CLIENT_MESSAGE_EVENT, Atom, ConnectionExt as _, ClientMessageEvent, EventMask, Window}}, rust_connection::RustConnection,
};

use smithay::reexports::calloop::{
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
pub struct X11Source {
    channel: Channel<Event>,
}

impl X11Source {
    /// Create a new X11 source.
    pub fn new(connection: Arc<RustConnection>, log: slog::Logger) -> Self {
        let (sender, channel) = sync_channel(5);
        let conn = Arc::clone(&connection);
        let event_thread = Some(spawn(move || {
            run_event_thread(connection, sender, log);
        }));
        Self { channel }
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
        self.channel.process_events(readiness, token, move |event, meta| {
            match event {
                ChannelEvent::Closed => slog::warn!(log, "Event thread exited"),
                ChannelEvent::Msg(event) => callback(event, meta)
            }
        })
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> IOResult<()> {
        self.channel.register(poll, factory)
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> IOResult<()> {
        self.channel.reregister(poll, factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> IOResult<()> {
        self.channel.unregister(poll)
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
            Ok(()) => {},
            Err(_) => {
                // The only possible error is that the other end of the channel was dropped.
                // This code should be unreachable, because X11Source owns the channel and waits
                // for this thread to exit in its Drop implementation.
                slog::info!(log, "Event thread exiting due to send error");
                break;
            }
        }
    }
}
