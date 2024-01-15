// TODO calloop source
// - poll POLLHUP/POLLIN on close_fd
// - poll for accept

use calloop::{
    generic::Generic, EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory,
};
use std::{
    io,
    os::unix::{
        io::OwnedFd,
        net::{UnixListener, UnixStream},
    },
};

/// Security context listener event source.
///
/// This implements [`EventSource`] and may be inserted into an event loop.
#[derive(Debug)]
pub struct SecurityContextListenerSource {
    listen_fd: Generic<UnixListener>,
    close_fd: Generic<OwnedFd>,
}

impl SecurityContextListenerSource {
    pub(super) fn new(listen_fd: UnixListener, close_fd: OwnedFd) -> io::Result<Self> {
        listen_fd.set_nonblocking(true)?;
        let listen_fd = Generic::new(listen_fd, Interest::READ, Mode::Level);
        // close_fd.set_nonblocking(true);
        // XXX POLLHUP
        let close_fd = Generic::new(close_fd, Interest::READ, Mode::Level);
        Ok(Self { listen_fd, close_fd })
    }
}

impl EventSource for SecurityContextListenerSource {
    type Event = UnixStream;
    type Metadata = ();
    type Ret = ();
    type Error = io::Error;

    fn process_events<F: FnMut(Self::Event, &mut ())>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> io::Result<PostAction> {
        self.listen_fd.process_events(readiness, token, |_, socket| {
            loop {
                match socket.accept() {
                    Ok((stream, _)) => callback(stream, &mut ()),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        break;
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
            Ok(PostAction::Continue)
        })?;

        self.close_fd
            .process_events(readiness, token, |_, _fd| Ok(PostAction::Remove))
    }

    fn register(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> calloop::Result<()> {
        self.listen_fd.register(poll, token_factory)?;
        self.close_fd.register(poll, token_factory)
    }

    fn reregister(&mut self, poll: &mut Poll, token_factory: &mut TokenFactory) -> calloop::Result<()> {
        self.listen_fd.reregister(poll, token_factory)?;
        self.close_fd.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.close_fd.unregister(poll)?;
        self.listen_fd.unregister(poll)
    }
}
