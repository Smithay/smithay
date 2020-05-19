use std::io;

use calloop::{EventSource, Interest, Mode, Poll, Readiness, Token};

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
}

impl DBusConnection {
    pub fn new_system() -> Result<DBusConnection, dbus::Error> {
        let mut chan = Channel::get_private(BusType::System)?;
        chan.set_watch_enabled(true);
        Ok(DBusConnection {
            cx: chan.into(),
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

    fn process_events<F>(&mut self, _: Readiness, _: Token, mut callback: F) -> io::Result<()>
    where
        F: FnMut(Message, &mut DBusConnection) -> (),
    {
        self.cx
            .channel()
            .read_write(Some(std::time::Duration::from_millis(0)))
            .map_err(|()| io::Error::new(io::ErrorKind::NotConnected, "DBus connection is closed"))?;
        while let Some(message) = self.cx.channel().pop_message() {
            callback(message, self);
        }
        self.cx.channel().flush();
        Ok(())
    }

    fn register(&mut self, poll: &mut Poll, token: Token) -> io::Result<()> {
        if self.current_watch.read || self.current_watch.write {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "DBus session already registered to calloop",
            ));
        }
        // reregister handles all the watch logic
        self.reregister(poll, token)
    }

    fn reregister(&mut self, poll: &mut Poll, token: Token) -> io::Result<()> {
        let new_watch = self.cx.channel().watch();
        let new_interest = match (new_watch.read, new_watch.write) {
            (true, true) => Some(Interest::Both),
            (true, false) => Some(Interest::Readable),
            (false, true) => Some(Interest::Writable),
            (false, false) => None,
        };
        if new_watch.fd != self.current_watch.fd {
            // remove the previous fd
            if self.current_watch.read || self.current_watch.write {
                poll.unregister(self.current_watch.fd)?;
            }
            // insert the new one
            if let Some(interest) = new_interest {
                poll.register(new_watch.fd, interest, Mode::Level, token)?;
            }
        } else {
            // update the registration
            if let Some(interest) = new_interest {
                poll.reregister(self.current_watch.fd, interest, Mode::Level, token)?;
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
        self.current_watch = Watch {
            fd: -1,
            read: false,
            write: false,
        };
        Ok(())
    }
}
