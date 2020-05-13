//!
//! Support to register a [`GbmDevice`](GbmDevice)
//! to an open [`Session`](::backend::session::Session).
//!

use drm::control::crtc;
use gbm::BufferObject;
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::rc::{Rc, Weak};

use super::{GbmDevice, GbmSurfaceInternal};
use crate::backend::drm::{RawDevice, RawSurface};
use crate::backend::graphics::CursorBackend;
use crate::backend::session::{AsSessionObserver, SessionObserver};

/// [`SessionObserver`](SessionObserver)
/// linked to the [`GbmDevice`](GbmDevice) it was
/// created from.
pub struct GbmDeviceObserver<
    S: SessionObserver + 'static,
    D: RawDevice + ::drm::control::Device + AsSessionObserver<S> + 'static,
> {
    observer: S,
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<GbmSurfaceInternal<D>>>>>,
    logger: ::slog::Logger,
}

impl<
        O: SessionObserver + 'static,
        S: CursorBackend<CursorFormat = dyn drm::buffer::Buffer> + RawSurface + 'static,
        D: RawDevice<Surface = S> + drm::control::Device + AsSessionObserver<O> + 'static,
    > AsSessionObserver<GbmDeviceObserver<O, D>> for GbmDevice<D>
{
    fn observer(&mut self) -> GbmDeviceObserver<O, D> {
        GbmDeviceObserver {
            observer: (**self.dev.borrow_mut()).observer(),
            backends: Rc::downgrade(&self.backends),
            logger: self.logger.clone(),
        }
    }
}

impl<
        O: SessionObserver + 'static,
        S: CursorBackend<CursorFormat = dyn drm::buffer::Buffer> + RawSurface + 'static,
        D: RawDevice<Surface = S> + drm::control::Device + AsSessionObserver<O> + 'static,
    > SessionObserver for GbmDeviceObserver<O, D>
{
    fn pause(&mut self, devnum: Option<(u32, u32)>) {
        self.observer.pause(devnum);
    }

    fn activate(&mut self, devnum: Option<(u32, u32, Option<RawFd>)>) {
        self.observer.activate(devnum);
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
