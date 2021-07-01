//!
//! Implementation of the [`Session`](::backend::session::Session) trait through the libseat.
//!
//! This requires libseat to be available on the system.

use libseat::{Seat, SeatEvent};
use std::{
    cell::RefCell,
    collections::HashMap,
    os::unix::io::RawFd,
    path::Path,
    rc::{Rc, Weak},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use nix::{errno::Errno, fcntl::OFlag, unistd::close};

use calloop::{EventSource, Poll, Readiness, Token};

use crate::{
    backend::session::{AsErrno, Session, Signal as SessionSignal},
    signaling::Signaler,
};

use slog::{debug, error, o};

#[derive(Debug)]
struct LibSeatSessionImpl {
    seat: RefCell<Seat>,
    active: Arc<AtomicBool>,
    devices: RefCell<HashMap<RawFd, i32>>,
    logger: ::slog::Logger,
}

impl Drop for LibSeatSessionImpl {
    fn drop(&mut self) {
        debug!(self.logger, "Closing seat")
    }
}

/// [`Session`] via the libseat
#[derive(Debug, Clone)]
pub struct LibSeatSession {
    internal: Weak<LibSeatSessionImpl>,
    seat_name: String,
}

/// [`SessionNotifier`] via the libseat
#[derive(Debug)]
pub struct LibSeatSessionNotifier {
    internal: Rc<LibSeatSessionImpl>,
    signaler: Signaler<SessionSignal>,
}

impl LibSeatSession {
    /// Tries to create a new session via libseat.
    pub fn new<L>(logger: L) -> Result<(LibSeatSession, LibSeatSessionNotifier), Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let logger = crate::slog_or_fallback(logger)
            .new(o!("smithay_module" => "backend_session", "session_type" => "libseat"));

        let active = Arc::new(AtomicBool::new(false));
        let signaler = Signaler::new();

        let seat = {
            let log = logger.clone();
            let active = active.clone();
            let signaler = signaler.clone();

            Seat::open(
                move |seat, event| match event {
                    SeatEvent::Enable => {
                        debug!(log, "Enable callback called");
                        active.store(true, Ordering::SeqCst);
                        signaler.signal(SessionSignal::ActivateSession);
                    }
                    SeatEvent::Disable => {
                        debug!(log, "Disable callback called");
                        active.store(false, Ordering::SeqCst);
                        signaler.signal(SessionSignal::PauseSession);
                        seat.disable().unwrap();
                    }
                },
                logger.clone(),
            )
        };

        seat.map(|mut seat| {
            // In some cases enable_seat event is avalible right after startup
            // so, we can dispatch it
            seat.dispatch(0).unwrap();

            let seat_name = seat.name().to_owned();

            let internal = Rc::new(LibSeatSessionImpl {
                seat: RefCell::new(seat),
                active,
                devices: RefCell::new(HashMap::new()),
                logger,
            });

            (
                LibSeatSession {
                    internal: Rc::downgrade(&internal),
                    seat_name,
                },
                LibSeatSessionNotifier { internal, signaler },
            )
        })
        .map_err(|err| Error::FailedToOpenSession(Errno::from_i32(err.into())))
    }
}

impl Session for LibSeatSession {
    type Error = Error;

    fn open(&mut self, path: &Path, _flags: OFlag) -> Result<RawFd, Self::Error> {
        if let Some(session) = self.internal.upgrade() {
            debug!(session.logger, "Opening device: {:?}", path);

            session
                .seat
                .borrow_mut()
                .open_device(&path)
                .map(|(id, fd)| {
                    session.devices.borrow_mut().insert(fd, id);
                    fd
                })
                .map_err(|err| Error::FailedToOpenDevice(Errno::from_i32(err.into())))
        } else {
            Err(Error::SessionLost)
        }
    }

    fn close(&mut self, fd: RawFd) -> Result<(), Self::Error> {
        if let Some(session) = self.internal.upgrade() {
            debug!(session.logger, "Closing device: {:?}", fd);

            let dev = session.devices.borrow().get(&fd).map(|fd| *fd);

            let out = if let Some(dev) = dev {
                session
                    .seat
                    .borrow_mut()
                    .close_device(dev)
                    .map_err(|err| Error::FailedToCloseDevice(Errno::from_i32(err.into())))
            } else {
                Ok(())
            };

            close(fd).unwrap();

            out
        } else {
            Err(Error::SessionLost)
        }
    }

    fn change_vt(&mut self, vt: i32) -> Result<(), Self::Error> {
        if let Some(session) = self.internal.upgrade() {
            debug!(session.logger, "Session switch: {:?}", vt);
            session
                .seat
                .borrow_mut()
                .switch_session(vt)
                .map_err(|err| Error::FailedToChangeVt(Errno::from_i32(err.into())))
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
        self.seat_name.clone()
    }
}

impl LibSeatSessionNotifier {
    /// Creates a new session object belonging to this notifier.
    pub fn session(&self) -> LibSeatSession {
        LibSeatSession {
            internal: Rc::downgrade(&self.internal),
            seat_name: self.internal.seat.borrow_mut().name().to_owned(),
        }
    }

    /// Get a handle to the Signaler of this session.
    ///
    /// You can use it to listen for signals generated by the session.
    pub fn signaler(&self) -> Signaler<SessionSignal> {
        self.signaler.clone()
    }
}

impl EventSource for LibSeatSessionNotifier {
    type Event = ();
    type Metadata = ();
    type Ret = ();

    fn process_events<F>(&mut self, _readiness: Readiness, _token: Token, _: F) -> std::io::Result<()>
    where
        F: FnMut((), &mut ()),
    {
        self.internal.seat.borrow_mut().dispatch(0).unwrap();
        Ok(())
    }

    fn register(&mut self, poll: &mut Poll, token: Token) -> std::io::Result<()> {
        poll.register(
            self.internal.seat.borrow_mut().get_fd().unwrap(),
            calloop::Interest::READ,
            calloop::Mode::Level,
            token,
        )
        .unwrap();
        Ok(())
    }

    fn reregister(&mut self, poll: &mut Poll, token: Token) -> std::io::Result<()> {
        poll.reregister(
            self.internal.seat.borrow_mut().get_fd().unwrap(),
            calloop::Interest::READ,
            calloop::Mode::Level,
            token,
        )
        .unwrap();
        Ok(())
    }

    fn unregister(&mut self, poll: &mut Poll) -> std::io::Result<()> {
        poll.unregister(self.internal.seat.borrow_mut().get_fd().unwrap())
            .unwrap();
        Ok(())
    }
}

/// Errors related to direct/tty sessions
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Failed to open session
    #[error("Failed to open session: {0}")]
    FailedToOpenSession(Errno),

    /// Failed to open device
    #[error("Failed to open device: {0}")]
    FailedToOpenDevice(Errno),

    /// Failed to close device
    #[error("Failed to close device: {0}")]
    FailedToCloseDevice(Errno),

    /// Failed to close device
    #[error("Failed to change vt: {0}")]
    FailedToChangeVt(Errno),

    /// Session is already closed,
    #[error("Session is already closed")]
    SessionLost,
}

impl AsErrno for Error {
    fn as_errno(&self) -> Option<i32> {
        match self {
            &Self::FailedToOpenSession(errno)
            | &Self::FailedToOpenDevice(errno)
            | &Self::FailedToCloseDevice(errno)
            | &Self::FailedToChangeVt(errno) => Some(errno as i32),
            _ => None,
        }
    }
}
