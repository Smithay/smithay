//!
//! Support to register a [`GbmDevice`](GbmDevice)
//! to an open [`Session`](::backend::session::Session).
//!

use drm::control::crtc;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use super::{GbmDevice, SurfaceInternalRef};
use crate::backend::drm::{Device, RawDevice};
use crate::backend::graphics::CursorBackend;
use crate::{
    backend::session::Signal as SessionSignal,
    signaling::{Linkable, Signaler},
};

/// [`SessionObserver`](SessionObserver)
/// linked to the [`GbmDevice`](GbmDevice) it was
/// created from.
pub(crate) struct GbmDeviceObserver<D: RawDevice + ::drm::control::Device + 'static> {
    backends: Weak<RefCell<HashMap<crtc::Handle, SurfaceInternalRef<D>>>>,
    _logger: ::slog::Logger,
}

impl<D> Linkable<SessionSignal> for GbmDevice<D>
where
    D: RawDevice + drm::control::Device + Linkable<SessionSignal> + 'static,
    <D as Device>::Surface: CursorBackend<CursorFormat = dyn drm::buffer::Buffer>,
{
    fn link(&mut self, signaler: Signaler<SessionSignal>) {
        let lower_signal = Signaler::new();
        self.raw.link(lower_signal.clone());
        let mut observer = GbmDeviceObserver::<D> {
            backends: Rc::downgrade(&self.backends),
            _logger: self.logger.clone(),
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

impl<D: RawDevice + drm::control::Device + 'static> GbmDeviceObserver<D>
where
    <D as Device>::Surface: CursorBackend<CursorFormat = dyn drm::buffer::Buffer>,
{
    fn activate(&mut self) {
        let mut crtcs = Vec::new();
        if let Some(backends) = self.backends.upgrade() {
            for (crtc, backend) in backends.borrow().iter() {
                if let Some(backend) = backend.upgrade() {
                    backend.clear_framebuffers();

                    // reset cursor
                    {
                        let cursor = backend.cursor.lock().unwrap();
                        let _ = backend.crtc.set_cursor_representation(&cursor.0, cursor.1);
                    }
                } else {
                    crtcs.push(*crtc);
                }
            }
            for crtc in crtcs {
                backends.borrow_mut().remove(&crtc);
            }
        }
    }
}
