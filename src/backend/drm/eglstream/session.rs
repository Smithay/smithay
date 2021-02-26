//!
//! Support to register a [`EglStreamDevice`](EglStreamDevice)
//! to an open [`Session`](::backend::session::Session).
//!

use super::{EglStreamDevice, EglStreamSurfaceInternal};
use crate::backend::drm::{Device, RawDevice, RawSurface};
use crate::backend::egl::ffi;
use crate::backend::session::Signal as SessionSignal;
use crate::signaling::{Linkable, Signaler};

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Weak as WeakArc;

use drm::control::{crtc, Device as ControlDevice};

/// [`SessionObserver`](SessionObserver)
/// linked to the [`EglStreamDevice`](EglStreamDevice) it was
/// created from.
#[derive(Debug)]
pub struct EglStreamDeviceObserver<S: RawSurface + 'static> {
    backends: Weak<RefCell<HashMap<crtc::Handle, WeakArc<EglStreamSurfaceInternal<S>>>>>,
    logger: ::slog::Logger,
}

impl<D> Linkable<SessionSignal> for EglStreamDevice<D>
where
    D: RawDevice + ControlDevice + Linkable<SessionSignal> + 'static,
{
    fn link(&mut self, signaler: Signaler<SessionSignal>) {
        let lower_signal = Signaler::new();
        self.raw.link(lower_signal.clone());
        let mut observer = EglStreamDeviceObserver::<<D as Device>::Surface> {
            backends: Rc::downgrade(&self.backends),
            logger: self.logger.clone(),
        };

        let token = signaler.register(move |&signal| match signal {
            SessionSignal::ActivateSession | SessionSignal::ActivateDevice { .. } => {
                // activate lower device *before* we process the signal
                lower_signal.signal(signal);
                observer.activate();
            }
            SessionSignal::PauseSession | SessionSignal::PauseDevice { .. } => {
                // pause lower device *after* we process the signal
                observer.pause();
                lower_signal.signal(signal);
            }
        });

        self.links.push(token);
    }
}

impl<S: RawSurface + 'static> EglStreamDeviceObserver<S> {
    fn pause(&mut self) {
        if let Some(backends) = self.backends.upgrade() {
            for (_, backend) in backends.borrow().iter() {
                if let Some(backend) = backend.upgrade() {
                    // destroy/disable the streams so it will not submit any pending frames
                    if let Some((display, stream)) = backend.stream.lock().unwrap().take() {
                        unsafe {
                            ffi::egl::DestroyStreamKHR(display.handle, stream.0);
                        }
                    }
                    // framebuffers will be likely not valid anymore, lets just recreate those after activation.
                    if let Some((buffer, fb)) = backend.commit_buffer.lock().unwrap().take() {
                        let _ = backend.crtc.destroy_framebuffer(fb);
                        let _ = backend.crtc.destroy_dumb_buffer(buffer);
                    }
                }
            }
        }
    }

    fn activate(&mut self) {
        if let Some(backends) = self.backends.upgrade() {
            for (_, backend) in backends.borrow().iter() {
                if let Some(backend) = backend.upgrade() {
                    let cursor = backend.cursor.lock().unwrap();
                    if let Some((ref cursor, ref hotspot)) = &*cursor {
                        if backend
                            .crtc
                            .set_cursor2(
                                backend.crtc.crtc(),
                                Some(cursor),
                                (hotspot.0 as i32, hotspot.1 as i32),
                            )
                            .is_err()
                        {
                            if let Err(err) = backend.crtc.set_cursor(backend.crtc.crtc(), Some(cursor)) {
                                warn!(self.logger, "Failed to reset cursor: {}", err)
                            }
                        }
                    }
                }
            }
        }
    }
}
