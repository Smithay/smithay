use std::{
    io::{Error as IOError, ErrorKind, Result as IOResult},
    os::unix::io::AsRawFd,
    rc::Rc,
};

use x11rb::{
    connection::Connection as _, errors::ReplyOrIdError, protocol::Event, rust_connection::RustConnection,
};

use smithay::reexports::calloop::{
    generic::{Fd, Generic},
    EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory,
};

pub struct X11Source {
    connection: Rc<RustConnection>,
    generic: Generic<Fd>,
}

impl X11Source {
    pub fn new(connection: Rc<RustConnection>) -> Self {
        let fd = Fd(connection.stream().as_raw_fd());
        let generic = Generic::new(fd, Interest::READ, Mode::Level);
        Self { connection, generic }
    }
}

impl EventSource for X11Source {
    type Event = Vec<Event>;
    type Metadata = ();
    type Ret = Result<(), ReplyOrIdError>;

    fn process_events<C>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: C,
    ) -> IOResult<PostAction>
    where
        C: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        fn inner<C>(conn: &RustConnection, mut callback: C) -> Result<(), ReplyOrIdError>
        where
            C: FnMut(Vec<Event>, &mut ()) -> Result<(), ReplyOrIdError>,
        {
            let mut events = Vec::new();
            while let Some(event) = conn.poll_for_event()? {
                events.push(event);
            }
            if !events.is_empty() {
                callback(events, &mut ())?;
            }
            conn.flush()?;
            Ok(())
        }
        let connection = &self.connection;
        self.generic.process_events(readiness, token, |_, _| {
            inner(connection, &mut callback).map_err(|err| IOError::new(ErrorKind::Other, err))?;
            Ok(PostAction::Continue)
        })
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> IOResult<()> {
        self.generic.register(poll, factory)
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> IOResult<()> {
        self.generic.reregister(poll, factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> IOResult<()> {
        self.generic.unregister(poll)
    }
}
