//!
//! Support to register an [`EglDevice`](EglDevice)
//! to an open [`Session`](::backend::session::Session).
//!

use drm::control::{connector, crtc, Mode};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::{atomic::Ordering, Weak as WeakArc};

use super::{EglDevice, EglSurfaceInternal};
use crate::backend::drm::{Device, Surface};
use crate::backend::egl::{
    ffi,
    native::{Backend, NativeDisplay, NativeSurface},
};
use crate::{
    backend::session::Signal as SessionSignal,
    signaling::{Linkable, Signaler},
};

/// [`SessionObserver`](SessionObserver)
/// linked to the [`EglDevice`](EglDevice) it was
/// created from.
pub struct EglDeviceObserver<N: NativeSurface + Surface> {
    backends: Weak<RefCell<HashMap<crtc::Handle, WeakArc<EglSurfaceInternal<N>>>>>,
}

impl<B, D> Linkable<SessionSignal> for EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface, Error = <<D as Device>::Surface as Surface>::Error>
        + 'static,
    D: Device
        + NativeDisplay<B, Arguments = (crtc::Handle, Mode, Vec<connector::Handle>)>
        + Linkable<SessionSignal>
        + 'static,
    <D as Device>::Surface: NativeSurface<Error = <<D as Device>::Surface as Surface>::Error>,
{
    fn link(&mut self, signaler: Signaler<SessionSignal>) {
        let lower_signal = Signaler::new();
        self.dev.borrow_mut().link(lower_signal.clone());
        let mut observer = EglDeviceObserver {
            backends: Rc::downgrade(&self.backends),
        };

        let token = signaler.register(move |&signal| match signal {
            SessionSignal::ActivateSession | SessionSignal::ActivateDevice { .. } => {
                // Activate lower *before* we process the event
                lower_signal.signal(signal);
                observer.activate()
            }
            _ => {
                lower_signal.signal(signal);
            }
        });

        self.links.push(token);
    }
}

impl<N: NativeSurface + Surface> EglDeviceObserver<N> {
    fn activate(&mut self) {
        if let Some(backends) = self.backends.upgrade() {
            for (_crtc, backend) in backends.borrow().iter() {
                if let Some(backend) = backend.upgrade() {
                    let old_surface = backend
                        .surface
                        .surface
                        .swap(std::ptr::null_mut(), Ordering::SeqCst);
                    if !old_surface.is_null() {
                        unsafe {
                            ffi::egl::DestroySurface(**backend.surface.display, old_surface as *const _);
                        }
                    }
                }
            }
        }
    }
}
