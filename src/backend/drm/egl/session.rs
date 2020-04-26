//!
//! Support to register an [`EglDevice`](EglDevice)
//! to an open [`Session`](::backend::session::Session).
//!

use drm::control::{crtc, connector, Mode};
use std::os::unix::io::RawFd;

use super::EglDevice;
use crate::backend::drm::Device;
use crate::backend::egl::native::{Backend, NativeDisplay, NativeSurface};
use crate::backend::session::{AsSessionObserver, SessionObserver};

/// [`SessionObserver`](SessionObserver)
/// linked to the [`EglDevice`](EglDevice) it was
/// created from.
pub struct EglDeviceObserver<S: SessionObserver + 'static> {
    observer: S,
}

impl<S, B, D> AsSessionObserver<EglDeviceObserver<S>> for EglDevice<B, D>
where
    S: SessionObserver + 'static,
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B, Arguments = (crtc::Handle, Mode, Vec<connector::Handle>)> + AsSessionObserver<S> + 'static,
    <D as Device>::Surface: NativeSurface,
{
    fn observer(&mut self) -> EglDeviceObserver<S> {
        EglDeviceObserver {
            observer: self.dev.borrow_mut().observer(),
        }
    }
}

impl<S: SessionObserver + 'static> SessionObserver for EglDeviceObserver<S> {
    fn pause(&mut self, devnum: Option<(u32, u32)>) {
        self.observer.pause(devnum);
    }

    fn activate(&mut self, devnum: Option<(u32, u32, Option<RawFd>)>) {
        self.observer.activate(devnum);
    }
}
