use std::path::Path;
use std::sync::{Arc, Mutex};
use std::rc::Rc;
use std::cell::RefCell;
use std::os::unix::io::RawFd;
use nix::fcntl::OFlag;
use wayland_server::StateProxy;

pub trait Session {
    type Error: AsErrno;

    fn open(&mut self, path: &Path, flags: OFlag) -> Result<RawFd, Self::Error>;
    fn close(&mut self, fd: RawFd) -> Result<(), Self::Error>;

    fn change_vt(&mut self, vt: i32) -> Result<(), Self::Error>;

    fn is_active(&self) -> bool;
    fn seat(&self) -> String;
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

    fn open(&mut self, _path: &Path, _flags: OFlag) -> Result<RawFd, Self::Error> { Err(()) }
    fn close(&mut self, _fd: RawFd) -> Result<(), Self::Error> { Err(()) }

    fn change_vt(&mut self, _vt: i32) -> Result<(), Self::Error> { Err(()) }

    fn is_active(&self) -> bool { false }
    fn seat(&self) -> String { String::from("seat0") }
}

impl<S: Session> Session for Rc<RefCell<S>> {
    type Error = S::Error;

    fn open(&mut self, path: &Path, flags: OFlag) -> Result<RawFd, Self::Error> {
        self.borrow_mut().open(path, flags)
    }

    fn close(&mut self, fd: RawFd) -> Result<(), Self::Error> {
        self.borrow_mut().close(fd)
    }

    fn change_vt(&mut self, vt: i32) -> Result<(), Self::Error> {
        self.borrow_mut().change_vt(vt)
    }

    fn is_active(&self) -> bool {
        self.borrow().is_active()
    }

    fn seat(&self) -> String {
        self.borrow().seat()
    }
}

impl<S: Session> Session for Arc<Mutex<S>> {
    type Error = S::Error;

    fn open(&mut self, path: &Path, flags: OFlag) -> Result<RawFd, Self::Error> {
        self.lock().unwrap().open(path, flags)
    }

    fn close(&mut self, fd: RawFd) -> Result<(), Self::Error> {
        self.lock().unwrap().close(fd)
    }

    fn change_vt(&mut self, vt: i32) -> Result<(), Self::Error> {
        self.lock().unwrap().change_vt(vt)
    }

    fn is_active(&self) -> bool {
        self.lock().unwrap().is_active()
    }

    fn seat(&self) -> String {
        self.lock().unwrap().seat()
    }
}

pub trait AsErrno: ::std::fmt::Debug {
    fn as_errno(&self) -> Option<i32>;
}

impl AsErrno for () {
    fn as_errno(&self) -> Option<i32> {
        None
    }
}

pub mod direct;
#[cfg(feature = "backend_session_logind")]
pub mod logind;
