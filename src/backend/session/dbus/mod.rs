use std::io;

use calloop::{EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory};

use dbus::{
    blocking::LocalConnection,
    channel::{BusType, Channel, Watch},
    Message,
};

#[cfg(feature = "backend_session_logind")]
pub mod logind;

/// An internal wrapper for handling a DBus connection
///
/// It acts as a calloop event source to dispatch the DBus events
pub(crate) struct DBusConnection {
    cx: LocalConnection,
    current_watch: Watch,
    token: Token,
}

impl DBusConnection {
    pub fn new_system() -> Result<DBusConnection, dbus::Error> {
        let mut chan = Channel::get_private(BusType::System)?;
        chan.set_watch_enabled(true);
        Ok(DBusConnection {
            cx: chan.into(),
            token: Token::invalid(),
            current_watch: Watch {
                fd: -1,
                read: false,
                write: false,
            },
        })
    }

    pub fn add_match(&self, match_str: &str) -> Result<(), dbus::Error> {
        self.cx.add_match_no_cb(match_str)
    }

    pub fn channel(&self) -> &Channel {
        self.cx.channel()
    }
}

impl EventSource for DBusConnection {
    type Event = Message;
    type Metadata = DBusConnection;
    type Ret = ();

    fn process_events<F>(&mut self, _: Readiness, token: Token, mut callback: F) -> io::Result<PostAction>
    where
        F: FnMut(Message, &mut DBusConnection),
    {
        if token != self.token {
            return Ok(PostAction::Continue);
        }
        self.cx
            .channel()
            .read_write(Some(std::time::Duration::from_millis(0)))
            .map_err(|()| io::Error::new(io::ErrorKind::NotConnected, "DBus connection is closed"))?;
        while let Some(message) = self.cx.channel().pop_message() {
            callback(message, self);
        }
        self.cx.channel().flush();
        Ok(PostAction::Continue)
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> io::Result<()> {
        if self.current_watch.read || self.current_watch.write {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "DBus session already registered to calloop",
            ));
        }
        // reregister handles all the watch logic
        self.reregister(poll, factory)
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> io::Result<()> {
        let new_watch = self.cx.channel().watch();
        let new_interest = match (new_watch.read, new_watch.write) {
            (true, true) => Some(Interest::BOTH),
            (true, false) => Some(Interest::READ),
            (false, true) => Some(Interest::WRITE),
            (false, false) => None,
        };
        self.token = factory.token();
        if new_watch.fd != self.current_watch.fd {
            // remove the previous fd
            if self.current_watch.read || self.current_watch.write {
                poll.unregister(self.current_watch.fd)?;
            }
            // insert the new one
            if let Some(interest) = new_interest {
                poll.register(new_watch.fd, interest, Mode::Level, self.token)?;
            }
        } else {
            // update the registration
            if let Some(interest) = new_interest {
                poll.reregister(self.current_watch.fd, interest, Mode::Level, self.token)?;
            } else {
                poll.unregister(self.current_watch.fd)?;
            }
        }
        self.current_watch = new_watch;
        Ok(())
    }

    fn unregister(&mut self, poll: &mut Poll) -> io::Result<()> {
        if self.current_watch.read || self.current_watch.write {
            poll.unregister(self.current_watch.fd)?;
        }
        self.token = Token::invalid();
        self.current_watch = Watch {
            fd: -1,
            read: false,
            write: false,
        };
        Ok(())
    }
}
