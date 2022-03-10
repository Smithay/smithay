//!
//! Implementation of the [`Session`](crate::backend::session::Session) trait through the logind dbus interface.
//!
//! This requires systemd and dbus to be available and started on the system.
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize a session just call [`LogindSession::new`].
//! A new session will be opened, if the call is successful and will be closed once the
//! [`LogindSessionNotifier`] is dropped.
//!
//! ### Usage of the session
//!
//! The session may be used to open devices manually through the [`Session`](crate::backend::session::Session) interface
//! or be passed to other objects that need it to open devices themselves.
//! The [`LogindSession`] is clonable
//! and may be passed to multiple devices easily.
//!
//! Examples for those are e.g. the [`LibinputInputBackend`](crate::backend::libinput::LibinputInputBackend)
//! (its context might be initialized through a [`Session`](crate::backend::session::Session) via the
//! [`LibinputSessionInterface`](crate::backend::libinput::LibinputSessionInterface)).
//!
//! ### Usage of the session notifier
//!
//! The notifier might be used to pause device access, when the session gets paused (e.g. by
//! switching the tty via [`LogindSession::change_vt`](crate::backend::session::Session::change_vt))
//! and to automatically enable it again, when the session becomes active again.
//!
//! It is crucial to avoid errors during that state. Examples for object that might be registered
//! for notifications are the [`Libinput`](input::Libinput) context or the [`DrmDevice`](crate::backend::drm::DrmDevice).
//!
//! The [`LogindSessionNotifier`] is to be inserted into
//! a calloop event source to have its events processed.

use crate::{
    backend::session::{AsErrno, Session, Signal as SessionSignal},
    utils::signaling::Signaler,
};
use dbus::{
    arg::{messageitem::MessageItem, OwnedFd},
    strings::{BusName, Interface, Member, Path as DbusPath},
    Message,
};
use nix::{
    fcntl::OFlag,
    sys::stat::{fstat, major, minor, stat},
};
use std::{
    cell::RefCell,
    fmt, io,
    os::unix::io::RawFd,
    path::Path,
    rc::{Rc, Weak},
    sync::atomic::{AtomicBool, Ordering},
};

use calloop::{EventSource, Poll, PostAction, Readiness, Token, TokenFactory};

use slog::{debug, error, info, o, warn};

use super::DBusConnection;

struct LogindSessionImpl {
    session_id: String,
    conn: RefCell<DBusConnection>,
    session_path: DbusPath<'static>,
    active: AtomicBool,
    signaler: Signaler<SessionSignal>,
    seat: String,
    logger: ::slog::Logger,
}

// DBusConnection does not implement debug, so we have to impl Debug manually
impl fmt::Debug for LogindSessionImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogindSessionImpl")
            .field("session_id", &self.session_id)
            .field("conn", &"...")
            .field("session_path", &self.session_path)
            .field("active", &self.active)
            .field("signaler", &self.signaler)
            .field("seat", &self.seat)
            .field("logger", &self.logger)
            .finish()
    }
}

/// [`Session`] via the logind dbus interface
#[derive(Debug, Clone)]
pub struct LogindSession {
    internal: Weak<LogindSessionImpl>,
    seat: String,
}

/// Notifier for the logind dbus interface
#[derive(Debug, Clone)]
pub struct LogindSessionNotifier {
    internal: Rc<LogindSessionImpl>,
}

impl LogindSession {
    /// Tries to create a new session via the logind dbus interface.
    pub fn new<L>(logger: L) -> Result<(LogindSession, LogindSessionNotifier), Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let logger = crate::slog_or_fallback(logger)
            .new(o!("smithay_module" => "backend_session", "session_type" => "logind"));

        // Acquire session_id, seat and vt (if any) via libsystemd
        let (session_id, seat, vt) = ffi::get_login_state()?;

        // Create dbus connection
        let conn = DBusConnection::new_system().map_err(Error::FailedDbusConnection)?;
        // and get the session path
        let session_path = LogindSessionImpl::blocking_call(
            &conn,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            "GetSession",
            Some(vec![session_id.clone().into()]),
        )?
        .get1::<DbusPath<'static>>()
        .ok_or(Error::UnexpectedMethodReturn)?;

        // Match all signals that we want to receive and handle
        let match1 = String::from(
            "type='signal',\
             sender='org.freedesktop.login1',\
             interface='org.freedesktop.login1.Manager',\
             member='SessionRemoved',\
             path='/org/freedesktop/login1'",
        );
        conn.add_match(&match1)
            .map_err(|source| Error::DbusMatchFailed(match1, source))?;
        let match2 = format!(
            "type='signal',\
             sender='org.freedesktop.login1',\
             interface='org.freedesktop.login1.Session',\
             member='PauseDevice',\
             path='{}'",
            &session_path
        );
        conn.add_match(&match2)
            .map_err(|source| Error::DbusMatchFailed(match2, source))?;
        let match3 = format!(
            "type='signal',\
             sender='org.freedesktop.login1',\
             interface='org.freedesktop.login1.Session',\
             member='ResumeDevice',\
             path='{}'",
            &session_path
        );
        conn.add_match(&match3)
            .map_err(|source| Error::DbusMatchFailed(match3, source))?;
        let match4 = format!(
            "type='signal',\
             sender='org.freedesktop.login1',\
             interface='org.freedesktop.DBus.Properties',\
             member='PropertiesChanged',\
             path='{}'",
            &session_path
        );
        conn.add_match(&match4)
            .map_err(|source| Error::DbusMatchFailed(match4, source))?;

        // Activate (switch to) the session and take control
        LogindSessionImpl::blocking_call(
            &conn,
            "org.freedesktop.login1",
            session_path.clone(),
            "org.freedesktop.login1.Session",
            "Activate",
            None,
        )?;
        LogindSessionImpl::blocking_call(
            &conn,
            "org.freedesktop.login1",
            session_path.clone(),
            "org.freedesktop.login1.Session",
            "TakeControl",
            Some(vec![false.into()]),
        )?;

        let conn = RefCell::new(conn);

        let internal = Rc::new(LogindSessionImpl {
            session_id: session_id.clone(),
            conn,
            session_path,
            active: AtomicBool::new(true),
            signaler: Signaler::new(),
            seat: seat.clone(),
            logger: logger.new(o!("id" => session_id, "seat" => seat.clone(), "vt" => format!("{:?}", &vt))),
        });

        Ok((
            LogindSession {
                internal: Rc::downgrade(&internal),
                seat,
            },
            LogindSessionNotifier { internal },
        ))
    }
}

impl LogindSessionNotifier {
    /// Creates a new session object belonging to this notifier.
    pub fn session(&self) -> LogindSession {
        LogindSession {
            internal: Rc::downgrade(&self.internal),
            seat: self.internal.seat.clone(),
        }
    }

    /// Get a handle to the Signaler of this session.
    ///
    /// You can use it to listen for signals generated by the session.
    pub fn signaler(&self) -> Signaler<SessionSignal> {
        self.internal.signaler.clone()
    }
}

impl LogindSessionImpl {
    fn blocking_call<'d, 'p, 'i, 'm, D, P, I, M>(
        conn: &DBusConnection,
        destination: D,
        path: P,
        interface: I,
        method: M,
        arguments: Option<Vec<MessageItem>>,
    ) -> Result<Message, Error>
    where
        D: Into<BusName<'d>>,
        P: Into<DbusPath<'p>>,
        I: Into<Interface<'i>>,
        M: Into<Member<'m>>,
    {
        let destination = destination.into().into_static();
        let path = path.into().into_static();
        let interface = interface.into().into_static();
        let method = method.into().into_static();

        let mut message = Message::method_call(&destination, &path, &interface, &method);

        if let Some(arguments) = arguments {
            message.append_items(&arguments)
        };

        let mut message = conn
            .channel()
            .send_with_reply_and_block(message, std::time::Duration::from_millis(1000))
            .map_err(|source| Error::FailedToSendDbusCall {
                bus: destination.clone(),
                path: path.clone(),
                interface: interface.clone(),
                member: method.clone(),
                source,
            })?;

        match message.as_result() {
            Ok(_) => Ok(message),
            Err(err) => Err(Error::DbusCallFailed {
                bus: destination.clone(),
                path: path.clone(),
                interface: interface.clone(),
                member: method.clone(),
                source: err,
            }),
        }
    }

    fn handle_message(&self, message: dbus::Message) -> Result<(), Error> {
        if &*message.interface().unwrap() == "org.freedesktop.login1.Manager"
            && &*message.member().unwrap() == "SessionRemoved"
            && message.get1::<String>().unwrap() == self.session_id
        {
            error!(self.logger, "Session got closed by logind");
            //Ok... now what?
            //This session will never live again, but the user maybe has other sessions open
            //So lets just put it to sleep.. forever
            self.signaler.signal(SessionSignal::PauseSession);
            self.active.store(false, Ordering::SeqCst);
            warn!(self.logger, "Session is now considered inactive");
        } else if &*message.interface().unwrap() == "org.freedesktop.login1.Session" {
            if &*message.member().unwrap() == "PauseDevice" {
                let (major, minor, pause_type) = message.get3::<u32, u32, String>();
                let major = major.ok_or(Error::UnexpectedMethodReturn)?;
                let minor = minor.ok_or(Error::UnexpectedMethodReturn)?;
                // From https://www.freedesktop.org/wiki/Software/systemd/logind/:
                //  `force` means the device got paused by logind already and this is only an
                //  asynchronous notification.
                //  `pause` means logind tries to pause the device and grants you limited amount
                //  of time to pause it. You must respond to this via PauseDeviceComplete().
                //  This synchronous pausing-mechanism is used for backwards-compatibility to VTs
                //  and logind is **free to not make use of it**.
                //  It is also free to send a forced PauseDevice if you don't respond in a timely manner
                //  (or for any other reason).
                let pause_type = pause_type.ok_or(Error::UnexpectedMethodReturn)?;
                debug!(
                    self.logger,
                    "Request of type \"{}\" to close device ({},{})", pause_type, major, minor
                );

                // gone means the device was unplugged from the system and you will no longer get any
                // notifications about it.
                // This is handled via udev and is not part of our session api.
                if pause_type != "gone" {
                    self.signaler.signal(SessionSignal::PauseDevice { major, minor });
                }
                // the other possible types are "force" or "gone" (unplugged),
                // both expect no acknowledgement (note even this is not *really* necessary,
                // logind would just timeout and send a "force" event. There is no way to
                // keep the device.)
                if pause_type == "pause" {
                    LogindSessionImpl::blocking_call(
                        &*self.conn.borrow(),
                        "org.freedesktop.login1",
                        self.session_path.clone(),
                        "org.freedesktop.login1.Session",
                        "PauseDeviceComplete",
                        Some(vec![major.into(), minor.into()]),
                    )?;
                }
            } else if &*message.member().unwrap() == "ResumeDevice" {
                let (major, minor, fd) = message.get3::<u32, u32, OwnedFd>();
                let major = major.ok_or(Error::UnexpectedMethodReturn)?;
                let minor = minor.ok_or(Error::UnexpectedMethodReturn)?;
                let fd = fd.ok_or(Error::UnexpectedMethodReturn)?.into_fd();
                debug!(self.logger, "Reactivating device ({},{})", major, minor);
                self.signaler.signal(SessionSignal::ActivateDevice {
                    major,
                    minor,
                    new_fd: Some(fd),
                });
            }
        } else if &*message.interface().unwrap() == "org.freedesktop.DBus.Properties"
            && &*message.member().unwrap() == "PropertiesChanged"
        {
            use dbus::arg::{Array, Dict, Get, Iter, Variant};

            let (_, changed, _) = message
                .get3::<String, Dict<'_, String, Variant<Iter<'_>>, Iter<'_>>, Array<'_, String, Iter<'_>>>();
            let mut changed = changed.ok_or(Error::UnexpectedMethodReturn)?;
            if let Some((_, mut value)) = changed.find(|&(ref key, _)| &*key == "Active") {
                if let Some(active) = Get::get(&mut value.0) {
                    self.active.store(active, Ordering::SeqCst);
                }
            }
        } else {
            // Handle default replies if necessary
            if let Some(reply) = dbus::channel::default_reply(&message) {
                self.conn
                    .borrow()
                    .channel()
                    .send(reply)
                    .map_err(|()| Error::SessionLost)?;
            }
        }
        Ok(())
    }
}

impl Session for LogindSession {
    type Error = Error;

    fn open(&mut self, path: &Path, _flags: OFlag) -> Result<RawFd, Error> {
        if let Some(session) = self.internal.upgrade() {
            let stat = stat(path).map_err(Error::FailedToStatDevice)?;
            // TODO handle paused
            let (fd, _paused) = LogindSessionImpl::blocking_call(
                &*session.conn.borrow(),
                "org.freedesktop.login1",
                session.session_path.clone(),
                "org.freedesktop.login1.Session",
                "TakeDevice",
                Some(vec![
                    (major(stat.st_rdev) as u32).into(),
                    (minor(stat.st_rdev) as u32).into(),
                ]),
            )?
            .get2::<OwnedFd, bool>();
            let fd = fd.ok_or(Error::UnexpectedMethodReturn)?.into_fd();
            Ok(fd)
        } else {
            Err(Error::SessionLost)
        }
    }

    fn close(&mut self, fd: RawFd) -> Result<(), Error> {
        if let Some(session) = self.internal.upgrade() {
            let stat = fstat(fd).map_err(Error::FailedToStatDevice)?;
            LogindSessionImpl::blocking_call(
                &*session.conn.borrow(),
                "org.freedesktop.login1",
                session.session_path.clone(),
                "org.freedesktop.login1.Session",
                "ReleaseDevice",
                Some(vec![
                    (major(stat.st_rdev) as u32).into(),
                    (minor(stat.st_rdev) as u32).into(),
                ]),
            )
            .map(|_| ())
        } else {
            Err(Error::SessionLost)
        }
    }

    fn is_active(&self) -> bool {
        if let Some(internal) = self.internal.upgrade() {
            internal.active.load(Ordering::SeqCst)
        } else {
            false
        }
    }

    fn seat(&self) -> String {
        self.seat.clone()
    }

    fn change_vt(&mut self, vt_num: i32) -> Result<(), Error> {
        if let Some(session) = self.internal.upgrade() {
            LogindSessionImpl::blocking_call(
                &*session.conn.borrow_mut(),
                "org.freedesktop.login1",
                "/org/freedesktop/login1/seat/self",
                "org.freedesktop.login1.Seat",
                "SwitchTo",
                Some(vec![(vt_num as u32).into()]),
            )
            .map(|_| ())
        } else {
            Err(Error::SessionLost)
        }
    }
}

impl Drop for LogindSessionNotifier {
    fn drop(&mut self) {
        info!(self.internal.logger, "Closing logind session");
        // Release control again and drop everything closing the connection
        let _ = LogindSessionImpl::blocking_call(
            &*self.internal.conn.borrow(),
            "org.freedesktop.login1",
            self.internal.session_path.clone(),
            "org.freedesktop.login1.Session",
            "ReleaseControl",
            None,
        );
    }
}

impl EventSource for LogindSessionNotifier {
    type Event = ();
    type Metadata = ();
    type Ret = ();

    fn process_events<F>(&mut self, readiness: Readiness, token: Token, _: F) -> std::io::Result<PostAction>
    where
        F: FnMut((), &mut ()),
    {
        // Accumulate the messages, and then process them, as we can't keep the borrow on the `DBusConnection`
        // while processing the messages
        let mut messages = Vec::new();
        self.internal
            .conn
            .borrow_mut()
            .process_events(readiness, token, |msg, _| messages.push(msg))?;

        for msg in messages {
            if let Err(err) = self.internal.handle_message(msg) {
                error!(self.internal.logger, "Error handling dbus messages: {}", err);
            }
        }

        Ok(PostAction::Continue)
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> std::io::Result<()> {
        self.internal.conn.borrow_mut().register(poll, factory)
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> std::io::Result<()> {
        self.internal.conn.borrow_mut().reregister(poll, factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> std::io::Result<()> {
        self.internal.conn.borrow_mut().unregister(poll)
    }
}

/// Errors related to logind sessions
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Failed to connect to dbus system socket
    #[error("Failed to connect to dbus system socket")]
    FailedDbusConnection(#[source] dbus::Error),
    /// Failed to get session from logind
    #[error("Failed to get session from logind")]
    FailedToGetSession(#[source] io::Error),
    /// Failed to get seat from logind
    #[error("Failed to get seat from logind")]
    FailedToGetSeat(#[source] io::Error),
    /// Failed to get vt from logind
    #[error("Failed to get vt from logind")]
    FailedToGetVT,
    /// Failed call to a dbus method
    #[error("Failed to call dbus method for service: {bus:?}, path: {path:?}, interface: {interface:?}, member: {member:?}")]
    FailedToSendDbusCall {
        /// Name of the service
        bus: BusName<'static>,
        /// Object path
        path: DbusPath<'static>,
        /// Interface
        interface: Interface<'static>,
        /// Method called
        member: Member<'static>,
        /// DBus error
        #[source]
        source: dbus::Error,
    },
    /// DBus method call failed
    #[error("Dbus message call failed for service: {bus:?}, path: {path:?}, interface: {interface:?}, member: {member:?}")]
    DbusCallFailed {
        /// Name of the service
        bus: BusName<'static>,
        /// Object path
        path: DbusPath<'static>,
        /// Interface
        interface: Interface<'static>,
        /// Method called
        member: Member<'static>,
        /// DBus error
        #[source]
        source: dbus::Error,
    },
    /// Dbus method return had unexpected format
    #[error("Dbus method return had unexpected format")]
    UnexpectedMethodReturn,
    /// Failed to setup dbus match rule
    #[error("Failed to setup dbus match rule {0}")]
    DbusMatchFailed(String, #[source] dbus::Error),
    /// Failed to stat device
    #[error("Failed to stat device")]
    FailedToStatDevice(#[source] nix::Error),
    /// Session is already closed,
    #[error("Session is already closed")]
    SessionLost,
}

impl AsErrno for Error {
    fn as_errno(&self) -> Option<i32> {
        None
    }
}

/*
 * FFI routines to retrieve the session state from logind or elogind
 */

mod ffi {
    use libc::pid_t;
    use std::{
        ffi::CString,
        os::raw::{c_char, c_int, c_uint},
    };

    pub fn get_login_state() -> Result<(String, String, Option<u32>), super::Error> {
        let session_name = unsafe {
            let mut session_name: *mut c_char = std::ptr::null_mut();
            let ret = sd_pid_get_session(0, &mut session_name);
            if ret < 0 {
                return Err(super::Error::FailedToGetSession(
                    std::io::Error::from_raw_os_error(-ret),
                ));
            }
            CString::from_raw(session_name)
        };

        let seat_name = unsafe {
            let mut seat_name: *mut c_char = std::ptr::null_mut();
            let ret = sd_session_get_seat(session_name.as_ptr(), &mut seat_name);
            if ret < 0 {
                return Err(super::Error::FailedToGetSeat(std::io::Error::from_raw_os_error(
                    -ret,
                )));
            }
            CString::from_raw(seat_name)
        };

        let vt_num = unsafe {
            let mut vt_num = 0;
            let ret = sd_session_get_vt(session_name.as_ptr(), &mut vt_num);
            if ret < 0 {
                None
            } else {
                Some(vt_num)
            }
        };

        Ok((
            session_name.into_string().unwrap(),
            seat_name.into_string().unwrap(),
            vt_num,
        ))
    }

    extern "C" {
        fn sd_pid_get_session(pid: pid_t, out_session: *mut *mut c_char) -> c_int;
        fn sd_session_get_seat(session: *const c_char, out_seat: *mut *mut c_char) -> c_int;
        fn sd_session_get_vt(session: *const c_char, out_vt: *mut c_uint) -> c_int;
    }
}
