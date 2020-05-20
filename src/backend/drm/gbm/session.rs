//!
//! Support to register a [`GbmDevice`](GbmDevice)
//! to an open [`Session`](::backend::session::Session).
//!

use drm::control::crtc;
use gbm::BufferObject;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use super::{GbmDevice, GbmSurfaceInternal};
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
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<GbmSurfaceInternal<D>>>>>,
    logger: ::slog::Logger,
}

impl<D> Linkable<SessionSignal> for GbmDevice<D>
where
    D: RawDevice + drm::control::Device + Linkable<SessionSignal> + 'static,
    <D as Device>::Surface: CursorBackend<CursorFormat = dyn drm::buffer::Buffer>,
{
    fn link(&mut self, signaler: Signaler<SessionSignal>) {
        let lower_signal = Signaler::new();
        self.dev.borrow_mut().link(lower_signal.clone());
        let mut observer = GbmDeviceObserver {
            backends: Rc::downgrade(&self.backends),
            logger: self.logger.clone(),
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
                        use ::drm::control::Device;

                        let &(ref cursor, ref hotspot): &(BufferObject<()>, (u32, u32)) =
                            unsafe { &*backend.cursor.as_ptr() };
                        if backend.crtc.set_cursor_representation(cursor, *hotspot).is_err() {
                            if let Err(err) = backend.dev.borrow().set_cursor(*crtc, Some(cursor)) {
                                error!(self.logger, "Failed to reset cursor. Error: {}", err);
                            }
                        }
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
