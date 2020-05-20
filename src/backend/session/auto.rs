//! Implementation of the [`Session`] trait through various implementations
//! automatically choosing the best available interface.
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize a session just call [`AutoSession::new`](::backend::session::auto::AutoSession::new).
//! A new session will be opened, if the any available interface is successful and will be closed once the
//! [`AutoSessionNotifier`](::backend::session::auto::AutoSessionNotifier) is dropped.
//!
//! ### Usage of the session
//!
//! The session may be used to open devices manually through the [`Session`] interface
//! or be passed to other objects that need it to open devices themselves.
//! The [`AutoSession`](::backend::session::auto::AutoSession) is clonable
//! and may be passed to multiple devices easily.
//!
//! Examples for those are e.g. the [`LibinputInputBackend`](::backend::libinput::LibinputInputBackend)
//! (its context might be initialized through a [`Session`] via the [`LibinputSessionInterface`](::backend::libinput::LibinputSessionInterface)).
//!
//! ### Usage of the session notifier
//!
//! The notifier might be used to pause device access, when the session gets paused (e.g. by
//! switching the tty via [`AutoSession::change_vt`](::backend::session::Session::change_vt))
//! and to automatically enable it again, when the session becomes active again.
//!
//! It is crucial to avoid errors during that state. Examples for object that might be registered
//! for notifications are the [`Libinput`](input::Libinput) context or the [`Device`](::backend::drm::Device).
//!
//! The [`AutoSessionNotifier`](::backend::session::auto::AutoSessionNotifier) is to be inserted into
//! a calloop event source to have its events processed.

#[cfg(feature = "backend_session_logind")]
use super::logind::{self, LogindSession, LogindSessionNotifier};
use super::{
    direct::{self, DirectSession, DirectSessionNotifier},
    AsErrno, Session, Signal as SessionSignal,
};
use crate::signaling::Signaler;
use nix::fcntl::OFlag;
use std::{cell::RefCell, io, os::unix::io::RawFd, path::Path, rc::Rc};

use calloop::{EventSource, Poll, Readiness, Token};

/// [`Session`] using the best available interface
#[derive(Clone)]
pub enum AutoSession {
    /// Logind session
    #[cfg(feature = "backend_session_logind")]
    Logind(LogindSession),
    /// Direct / tty session
    Direct(Rc<RefCell<DirectSession>>),
}

/// [`SessionNotifier`] using the best available interface
pub enum AutoSessionNotifier {
    /// Logind session notifier
    #[cfg(feature = "backend_session_logind")]
    Logind(LogindSessionNotifier),
    /// Direct / tty session notifier
    Direct(DirectSessionNotifier),
}

impl AutoSession {
    /// Tries to create a new session via the best available interface.
    #[cfg(feature = "backend_session_logind")]
    pub fn new<L>(logger: L) -> Option<(AutoSession, AutoSessionNotifier)>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let logger = crate::slog_or_stdlog(logger)
            .new(o!("smithay_module" => "backend_session_auto", "session_type" => "auto"));

        info!(logger, "Trying to create logind session");
        match LogindSession::new(logger.clone()) {
            Ok((session, notifier)) => Some((
                AutoSession::Logind(session),
                AutoSessionNotifier::Logind(notifier),
            )),
            Err(err) => {
                warn!(logger, "Failed to create logind session: {}", err);
                info!(logger, "Falling back to create tty session");
                match DirectSession::new(None, logger.clone()) {
                    Ok((session, notifier)) => Some((
                        AutoSession::Direct(Rc::new(RefCell::new(session))),
                        AutoSessionNotifier::Direct(notifier),
                    )),
                    Err(err) => {
                        warn!(logger, "Failed to create direct session: {}", err);
                        error!(logger, "Could not create any session, possibilities exhausted");
                        None
                    }
                }
            }
        }
    }

    /// Tries to create a new session via the best available interface.
    #[cfg(not(feature = "backend_session_logind"))]
    pub fn new<L>(logger: L) -> Option<(AutoSession, AutoSessionNotifier)>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let logger = crate::slog_or_stdlog(logger)
            .new(o!("smithay_module" => "backend_session_auto", "session_type" => "auto"));

        info!(logger, "Trying to create tty session");
        match DirectSession::new(None, logger.clone()) {
            Ok((session, notifier)) => Some((
                AutoSession::Direct(Rc::new(RefCell::new(session))),
                AutoSessionNotifier::Direct(notifier),
            )),
            Err(err) => {
                warn!(logger, "Failed to create direct session: {}", err);
                error!(logger, "Could not create any session, possibilities exhausted");
                None
            }
        }
    }
}

impl Session for AutoSession {
    type Error = Error;

    fn open(&mut self, path: &Path, flags: OFlag) -> Result<RawFd, Error> {
        match *self {
            #[cfg(feature = "backend_session_logind")]
            AutoSession::Logind(ref mut logind) => logind.open(path, flags).map_err(|e| e.into()),
            AutoSession::Direct(ref mut direct) => direct.open(path, flags).map_err(|e| e.into()),
        }
    }
    fn close(&mut self, fd: RawFd) -> Result<(), Error> {
        match *self {
            #[cfg(feature = "backend_session_logind")]
            AutoSession::Logind(ref mut logind) => logind.close(fd).map_err(|e| e.into()),
            AutoSession::Direct(ref mut direct) => direct.close(fd).map_err(|e| e.into()),
        }
    }

    fn change_vt(&mut self, vt: i32) -> Result<(), Error> {
        match *self {
            #[cfg(feature = "backend_session_logind")]
            AutoSession::Logind(ref mut logind) => logind.change_vt(vt).map_err(|e| e.into()),
            AutoSession::Direct(ref mut direct) => direct.change_vt(vt).map_err(|e| e.into()),
        }
    }

    fn is_active(&self) -> bool {
        match *self {
            #[cfg(feature = "backend_session_logind")]
            AutoSession::Logind(ref logind) => logind.is_active(),
            AutoSession::Direct(ref direct) => direct.is_active(),
        }
    }
    fn seat(&self) -> String {
        match *self {
            #[cfg(feature = "backend_session_logind")]
            AutoSession::Logind(ref logind) => logind.seat(),
            AutoSession::Direct(ref direct) => direct.seat(),
        }
    }
}

impl AutoSessionNotifier {
    /// Get a handle to the Signaler of this session.
    ///
    /// You can use it to listen for signals generated by the session.
    pub fn signaler(&self) -> Signaler<SessionSignal> {
        match *self {
            #[cfg(feature = "backend_session_logind")]
            AutoSessionNotifier::Logind(ref logind) => logind.signaler(),
            AutoSessionNotifier::Direct(ref direct) => direct.signaler(),
        }
    }
}

impl EventSource for AutoSessionNotifier {
    type Event = ();
    type Metadata = ();
    type Ret = ();

    fn process_events<F>(&mut self, readiness: Readiness, token: Token, callback: F) -> io::Result<()>
    where
        F: FnMut((), &mut ()),
    {
        match self {
            #[cfg(feature = "backend_session_logind")]
            AutoSessionNotifier::Logind(s) => s.process_events(readiness, token, callback),
            AutoSessionNotifier::Direct(s) => s.process_events(readiness, token, callback),
        }
    }

    fn register(&mut self, poll: &mut Poll, token: Token) -> io::Result<()> {
        match self {
            #[cfg(feature = "backend_session_logind")]
            AutoSessionNotifier::Logind(s) => EventSource::register(s, poll, token),
            AutoSessionNotifier::Direct(s) => EventSource::register(s, poll, token),
        }
    }

    fn reregister(&mut self, poll: &mut Poll, token: Token) -> io::Result<()> {
        match self {
            #[cfg(feature = "backend_session_logind")]
            AutoSessionNotifier::Logind(s) => EventSource::reregister(s, poll, token),
            AutoSessionNotifier::Direct(s) => EventSource::reregister(s, poll, token),
        }
    }

    fn unregister(&mut self, poll: &mut Poll) -> io::Result<()> {
        match self {
            #[cfg(feature = "backend_session_logind")]
            AutoSessionNotifier::Logind(s) => EventSource::unregister(s, poll),
            AutoSessionNotifier::Direct(s) => EventSource::unregister(s, poll),
        }
    }
}

/// Errors related to auto sessions
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[cfg(feature = "backend_session_logind")]
    /// Logind session error
    #[error("Logind session error: {0}")]
    Logind(#[from] logind::Error),
    /// Direct session error
    #[error("Direct session error: {0}")]
    Direct(#[from] direct::Error),
    /// Nix error
    #[error("Nix error: {0}")]
    Nix(#[from] nix::Error),
}

impl AsErrno for Error {
    fn as_errno(&self) -> Option<i32> {
        //TODO figure this out, I don't see a way..
        None
    }
}
