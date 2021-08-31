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

use calloop::{
    channel::{self, Channel},
    EventSource, Poll, PostAction, Readiness, Token, TokenFactory,
};

use crate::{
    backend::session::{AsErrno, Session, Signal as SessionSignal},
    utils::signaling::Signaler,
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
    rx: Channel<SeatEvent>,
    token: Token,
}

impl LibSeatSession {
    /// Tries to create a new session via libseat.
    pub fn new<L>(logger: L) -> Result<(LibSeatSession, LibSeatSessionNotifier), Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let logger = crate::slog_or_fallback(logger)
            .new(o!("smithay_module" => "backend_session", "session_type" => "libseat"));

        let (tx, rx) = calloop::channel::channel();

        let seat = {
            let log = logger.clone();

            Seat::open(
                move |_seat, event| match event {
                    SeatEvent::Enable => {
                        debug!(log, "Enable callback called");
                        tx.send(event).unwrap();
                    }
                    SeatEvent::Disable => {
                        debug!(log, "Disable callback called");
                        tx.send(event).unwrap();
                    }
                },
                logger.clone(),
            )
        };

        seat.map(|mut seat| {
            let seat_name = seat.name().to_owned();

            // In some cases enable_seat event is avalible right after startup
            // so, we can dispatch it
            seat.dispatch(0).unwrap();

            let internal = Rc::new(LibSeatSessionImpl {
                seat: RefCell::new(seat),
                active: Arc::new(AtomicBool::new(false)),
                devices: RefCell::new(HashMap::new()),
                logger,
            });

            let session = LibSeatSession {
                internal: Rc::downgrade(&internal),
                seat_name,
            };

            let notifier = LibSeatSessionNotifier {
                internal,
                signaler: Signaler::new(),
                rx,
                token: Token::invalid(),
            };

            (session, notifier)
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

            let dev = session.devices.borrow().get(&fd).copied();

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

    fn process_events<F>(&mut self, readiness: Readiness, token: Token, _: F) -> std::io::Result<PostAction>
    where
        F: FnMut((), &mut ()),
    {
        if token == self.token {
            self.internal.seat.borrow_mut().dispatch(0).unwrap();
        }

        let internal = &self.internal;
        let signaler = &self.signaler;

        self.rx.process_events(readiness, token, |event, _| match event {
            channel::Event::Msg(event) => match event {
                SeatEvent::Enable => {
                    internal.active.store(true, Ordering::SeqCst);
                    signaler.signal(SessionSignal::ActivateSession);
                }
                SeatEvent::Disable => {
                    internal.active.store(false, Ordering::SeqCst);
                    signaler.signal(SessionSignal::PauseSession);
                    internal.seat.borrow_mut().disable().unwrap();
                }
            },
            channel::Event::Closed => {
                // Tx is stored inside of Seat, and Rc<Seat> is stored in LibSeatSessionNotifier so this is unreachable
            }
        })
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> std::io::Result<()> {
        self.rx.register(poll, factory)?;

        self.token = factory.token();
        poll.register(
            self.internal.seat.borrow_mut().get_fd().unwrap(),
            calloop::Interest::READ,
            calloop::Mode::Level,
            self.token,
        )
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> std::io::Result<()> {
        self.rx.reregister(poll, factory)?;

        self.token = factory.token();
        poll.reregister(
            self.internal.seat.borrow_mut().get_fd().unwrap(),
            calloop::Interest::READ,
            calloop::Mode::Level,
            self.token,
        )
    }

    fn unregister(&mut self, poll: &mut Poll) -> std::io::Result<()> {
        self.rx.unregister(poll)?;

        self.token = Token::invalid();
        poll.unregister(self.internal.seat.borrow_mut().get_fd().unwrap())
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
