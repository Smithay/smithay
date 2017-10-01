use std::path::Path;
use std::os::unix::io::RawFd;
use wayland_server::StateProxy;

pub trait Session {
    type Error: ::std::fmt::Debug;

    fn open(&mut self, path: &Path) -> Result<RawFd, Self::Error>;
    fn close(&mut self, fd: RawFd) -> Result<(), Self::Error>;

    fn change_vt(&mut self, vt: i32) -> Result<(), Self::Error>;

    fn is_active(&self) -> bool;
    fn seat(&self) -> &str;
}

pub trait SessionNotifier {
    fn register<S: SessionObserver + 'static>(&mut self, signal: S) -> usize;
    fn unregister(&mut self, signal: usize);

    fn is_active(&self) -> bool;
    fn seat(&self) -> &str;
}

pub trait SessionObserver {
    fn pause<'a>(&mut self, state: &mut StateProxy<'a>);
    fn activate<'a>(&mut self, state: &mut StateProxy<'a>);
}

impl Session for () {
    type Error = ();

    fn open(&mut self, _path: &Path) -> Result<RawFd, Self::Error> { Err(()) }
    fn close(&mut self, _fd: RawFd) -> Result<(), Self::Error> { Err(()) }

    fn change_vt(&mut self, _vt: i32) -> Result<(), Self::Error> { Err(()) }

    fn is_active(&self) -> bool { false }
    fn seat(&self) -> &str { "seat0" }
}

pub mod direct;
#[cfg(feature = "backend_session_logind")]
pub mod logind;
