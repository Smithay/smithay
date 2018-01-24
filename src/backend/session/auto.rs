use std::io::{Result as IoResult};
use std::rc::Rc;
use std::cell::RefCell;
use std::os::unix::io::RawFd;
use std::path::Path;
use nix::fcntl::OFlag;
use wayland_server::{EventLoopHandle};
use wayland_server::sources::SignalEventSource;

use super::{Session, SessionNotifier, SessionObserver, AsErrno};
#[cfg(feature = "backend_session_logind")]
use super::logind::{self, LogindSession, LogindSessionNotifier, BoundLogindSession, logind_session_bind};
use super::direct::{self, DirectSession, DirectSessionNotifier, direct_session_bind};

#[derive(Clone)]
pub enum AutoSession {
    #[cfg(feature = "backend_session_logind")]
    Logind(LogindSession),
    Direct(Rc<RefCell<DirectSession>>),
}

pub enum AutoSessionNotifier {
    #[cfg(feature = "backend_session_logind")]
    Logind(LogindSessionNotifier),
    Direct(DirectSessionNotifier),
}

pub enum BoundAutoSession {
    #[cfg(feature = "backend_session_logind")]
    Logind(BoundLogindSession),
    Direct(SignalEventSource<DirectSessionNotifier>),
}

#[derive(PartialEq, Eq)]
pub struct AutoId(AutoIdInternal);
#[derive(PartialEq, Eq)]
enum AutoIdInternal {
    #[cfg(feature = "backend_session_logind")]
    Logind(logind::Id),
    Direct(direct::Id),
}

impl AutoSession {
    #[cfg(feature = "backend_session_logind")]
    pub fn new<L>(logger: L) -> Option<(AutoSession, AutoSessionNotifier)>
        where L: Into<Option<::slog::Logger>>
    {
        let logger = ::slog_or_stdlog(logger)
            .new(o!("smithay_module" => "backend_session_auto", "session_type" => "auto"));

        info!(logger, "Trying to create logind session");
        match LogindSession::new(logger.clone()) {
            Ok((session, notifier)) => Some((AutoSession::Logind(session), AutoSessionNotifier::Logind(notifier))),
            Err(err) => {
                warn!(logger, "Failed to create logind session: {}", err);
                info!(logger, "Falling back to create tty session");
                match DirectSession::new(None, logger.clone()) {
                    Ok((session, notifier)) => Some((AutoSession::Direct(Rc::new(RefCell::new(session))), AutoSessionNotifier::Direct(notifier))),
                    Err(err) => {
                        warn!(logger, "Failed to create direct session: {}", err);
                        error!(logger, "Could not create any session, possibilities exhausted");
                        None
                    }
                }
            }
        }
    }

    #[cfg(not(feature = "backend_session_logind"))]
    pub fn new<L>(logger: L) -> Option<(AutoSession, AutoSessionNotifier)>
        where L: Into<Option<::slog::Logger>>
    {
        let logger = ::slog_or_stdlog(logger)
            .new(o!("smithay_module" => "backend_session_auto", "session_type" => "auto"));

        info!(logger, "Trying to create tty session");
        match DirectSession::new(None, logger.clone()) {
            Ok((session, notifier)) => Some((AutoSession::Direct(Rc::new(RefCell::new(session))), AutoSessionNotifier::Direct(notifier))),
            Err(err) => {
                warn!(logger, "Failed to create direct session: {}", err);
                error!(logger, "Could not create any session, possibilities exhausted");
                None
            }
        }
    }
}

pub fn auto_session_bind(notifier: AutoSessionNotifier, evlh: &mut EventLoopHandle) -> IoResult<BoundAutoSession> {
    Ok(match notifier {
        #[cfg(feature = "backend_session_logind")]
        AutoSessionNotifier::Logind(logind) => BoundAutoSession::Logind(logind_session_bind(logind, evlh)?),
        AutoSessionNotifier::Direct(direct) => BoundAutoSession::Direct(direct_session_bind(direct, evlh)?),
    })
}

impl Session for AutoSession {
    type Error = Error;

    fn open(&mut self, path: &Path, flags: OFlag) -> Result<RawFd> {
        match self {
            #[cfg(feature = "backend_session_logind")]
            &mut AutoSession::Logind(ref mut logind) => logind.open(path, flags).map_err(|e| e.into()),
            &mut AutoSession::Direct(ref mut direct) => direct.open(path, flags).map_err(|e| e.into()),
        }
    }
    fn close(&mut self, fd: RawFd) -> Result<()> {
        match self {
            #[cfg(feature = "backend_session_logind")]
            &mut AutoSession::Logind(ref mut logind) => logind.close(fd).map_err(|e| e.into()),
            &mut AutoSession::Direct(ref mut direct) => direct.close(fd).map_err(|e| e.into()),
        }
    }

    fn change_vt(&mut self, vt: i32) -> Result<()> {
        match self {
            #[cfg(feature = "backend_session_logind")]
            &mut AutoSession::Logind(ref mut logind) => logind.change_vt(vt).map_err(|e| e.into()),
            &mut AutoSession::Direct(ref mut direct) => direct.change_vt(vt).map_err(|e| e.into()),
        }
    }

    fn is_active(&self) -> bool {
        match self {
            #[cfg(feature = "backend_session_logind")]
            &AutoSession::Logind(ref logind) => logind.is_active(),
            &AutoSession::Direct(ref direct) => direct.is_active(),
        }
    }
    fn seat(&self) -> String {
        match self {
            #[cfg(feature = "backend_session_logind")]
            &AutoSession::Logind(ref logind) => logind.seat(),
            &AutoSession::Direct(ref direct) => direct.seat(),
        }
    }
}

impl SessionNotifier for AutoSessionNotifier {
    type Id = AutoId;

    fn register<S: SessionObserver + 'static>(&mut self, signal: S) -> Self::Id {
        match self {
            #[cfg(feature = "backend_session_logind")]
            &mut AutoSessionNotifier::Logind(ref mut logind) => AutoId(AutoIdInternal::Logind(logind.register(signal))),
            &mut AutoSessionNotifier::Direct(ref mut direct) => AutoId(AutoIdInternal::Direct(direct.register(signal))),
        }
    }
    fn unregister(&mut self, signal: Self::Id) {
        match (self, signal) {
            #[cfg(feature = "backend_session_logind")]
            (&mut AutoSessionNotifier::Logind(ref mut logind), AutoId(AutoIdInternal::Logind(signal))) => logind.unregister(signal),
            (&mut AutoSessionNotifier::Direct(ref mut direct), AutoId(AutoIdInternal::Direct(signal))) => direct.unregister(signal),
            _ => unreachable!(),
        }
    }

    fn is_active(&self) -> bool {
        match self {
            #[cfg(feature = "backend_session_logind")]
            &AutoSessionNotifier::Logind(ref logind) => logind.is_active(),
            &AutoSessionNotifier::Direct(ref direct) => direct.is_active(),
        }
    }
    fn seat(&self) -> &str {
        match self {
            #[cfg(feature = "backend_session_logind")]
            &AutoSessionNotifier::Logind(ref logind) => logind.seat(),
            &AutoSessionNotifier::Direct(ref direct) => direct.seat(),
        }
    }
}

impl BoundAutoSession {
    pub fn remove(self) -> AutoSessionNotifier {
        match self {
            #[cfg(feature = "backend_session_logind")]
            BoundAutoSession::Logind(logind) => AutoSessionNotifier::Logind(logind.close()),
            BoundAutoSession::Direct(source) => AutoSessionNotifier::Direct(source.remove()),
        }
    }
}

error_chain! {
    links {
        Logind(logind::Error, logind::ErrorKind) #[cfg(feature = "backend_session_logind")];
    }

    foreign_links {
        Direct(::nix::Error);
    }
}

impl AsErrno for Error {
    fn as_errno(&self) -> Option<i32> {
        //TODO figure this out, I don't see a way..
        None
    }
}
