//! [`EventSource`] for polling a Wayland [`Display`].
//!
//! A [`PollingSource`] monitors Wayland display using it's poll file descriptor. When a client message is
//! received, an event will be dispatched in calloop. You should dispatch client messages using [`Display::dispatch_clients`]
//! when the callback is invoked.

use std::io;

use calloop::{
    generic::{Fd, Generic},
    EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory,
};
use wayland_server::Display;

/// An event source used to poll when there are incoming client messages.
#[derive(Debug)]
pub struct PollingSource {
    poll_fd: Generic<Fd>,
}

impl PollingSource {
    /// Creates a new [`PollingSource`] using the display's poll file descriptor.
    ///
    /// Generally you only need to create one source using the display.
    pub fn new<D: 'static>(display: &Display<D>) -> PollingSource {
        let backend = display.backend();
        let poll_fd = backend.lock().unwrap().poll_fd();

        PollingSource {
            poll_fd: Generic::from_fd(poll_fd, Interest::READ, Mode::Level),
        }
    }
}

impl EventSource for PollingSource {
    type Event = ();
    type Metadata = ();
    type Ret = io::Result<()>;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> io::Result<PostAction>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        callback((), &mut ())?;
        Ok(PostAction::Continue)
    }

    fn register(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> io::Result<()> {
        self.poll_fd.register(poll, token_factory)
    }

    fn reregister(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> io::Result<()> {
        self.poll_fd.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> io::Result<()> {
        self.poll_fd.unregister(poll)
    }
}
