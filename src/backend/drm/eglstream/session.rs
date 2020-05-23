//!
//! Support to register a [`EglStreamDevice`](EglStreamDevice)
//! to an open [`Session`](::backend::session::Session).
//!

use super::{EglStreamDevice, EglStreamSurfaceInternal};
use crate::backend::drm::{RawDevice, Surface};
use crate::backend::egl::ffi;
use crate::backend::session::{AsSessionObserver, SessionObserver};

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::rc::{Rc, Weak};

use drm::control::{crtc, Device as ControlDevice};

/// [`SessionObserver`](SessionObserver)
/// linked to the [`EglStreamDevice`](EglStreamDevice) it was
/// created from.
pub struct EglStreamDeviceObserver<
    O: SessionObserver + 'static,
    D: RawDevice + AsSessionObserver<O> + 'static,
> {
    observer: O,
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<EglStreamSurfaceInternal<D>>>>>,
    logger: ::slog::Logger,
}

impl<O: SessionObserver + 'static, D: RawDevice + ControlDevice + AsSessionObserver<O> + 'static>
    AsSessionObserver<EglStreamDeviceObserver<O, D>> for EglStreamDevice<D>
{
    fn observer(&mut self) -> EglStreamDeviceObserver<O, D> {
        EglStreamDeviceObserver {
            observer: self.raw.observer(),
            backends: Rc::downgrade(&self.backends),
            logger: self.logger.clone(),
        }
    }
}

impl<O: SessionObserver + 'static, D: RawDevice + AsSessionObserver<O> + 'static> SessionObserver
    for EglStreamDeviceObserver<O, D>
{
    fn pause(&mut self, devnum: Option<(u32, u32)>) {
        if let Some(backends) = self.backends.upgrade() {
            for (_, backend) in backends.borrow().iter() {
                if let Some(backend) = backend.upgrade() {
                    // destroy/disable the streams so it will not submit any pending frames
                    if let Some((display, stream)) = backend.stream.replace(None) {
                        unsafe {
                            ffi::egl::DestroyStreamKHR(display.handle, stream);
                        }
                    }
                    // framebuffers will be likely not valid anymore, lets just recreate those after activation.
                    if let Some((buffer, fb)) = backend.commit_buffer.take() {
                        let _ = backend.crtc.destroy_framebuffer(fb);
                        let _ = backend.crtc.destroy_dumb_buffer(buffer);
                    }
                }
            }
        }

        self.observer.pause(devnum);
    }

    fn activate(&mut self, devnum: Option<(u32, u32, Option<RawFd>)>) {
        self.observer.activate(devnum);
        if let Some(backends) = self.backends.upgrade() {
            for (_, backend) in backends.borrow().iter() {
                if let Some(backend) = backend.upgrade() {
                    if let Some((cursor, hotspot)) = backend.cursor.get() {
                        if backend
                            .crtc
                            .set_cursor2(
                                backend.crtc.crtc(),
                                Some(&cursor),
                                (hotspot.0 as i32, hotspot.1 as i32),
                            )
                            .is_err()
                        {
                            if let Err(err) = backend.crtc.set_cursor(backend.crtc.crtc(), Some(&cursor)) {
                                warn!(self.logger, "Failed to reset cursor: {}", err)
                            }
                        }
                    }
                }
            }
        }
    }
}
