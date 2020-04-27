//!
//! Support to register an [`EglDevice`](EglDevice)
//! to an open [`Session`](::backend::session::Session).
//!

use drm::control::{connector, crtc, Mode};
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::rc::{Rc, Weak};

use super::{EglDevice, EglSurfaceInternal};
use crate::backend::drm::{Device, Surface};
use crate::backend::egl::native::{Backend, NativeDisplay, NativeSurface};
use crate::backend::session::{AsSessionObserver, SessionObserver};

/// [`SessionObserver`](SessionObserver)
/// linked to the [`EglDevice`](EglDevice) it was
/// created from.
pub struct EglDeviceObserver<S: SessionObserver + 'static, N: NativeSurface + Surface> {
    observer: S,
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<EglSurfaceInternal<N>>>>>,
}

impl<S, B, D> AsSessionObserver<EglDeviceObserver<S, <D as Device>::Surface>> for EglDevice<B, D>
where
    S: SessionObserver + 'static,
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device
        + NativeDisplay<
            B,
            Arguments = (crtc::Handle, Mode, Vec<connector::Handle>),
            Error = <<D as Device>::Surface as Surface>::Error,
        > + AsSessionObserver<S>
        + 'static,
    <D as Device>::Surface: NativeSurface,
{
    fn observer(&mut self) -> EglDeviceObserver<S, <D as Device>::Surface> {
        EglDeviceObserver {
            observer: self.dev.borrow_mut().observer(),
            backends: Rc::downgrade(&self.backends),
        }
    }
}

impl<S: SessionObserver + 'static, N: NativeSurface + Surface> SessionObserver for EglDeviceObserver<S, N> {
    fn pause(&mut self, devnum: Option<(u32, u32)>) {
        self.observer.pause(devnum);
    }

    fn activate(&mut self, devnum: Option<(u32, u32, Option<RawFd>)>) {
        self.observer.activate(devnum);
    }
}
