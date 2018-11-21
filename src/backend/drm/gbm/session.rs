use drm::control::{crtc, Device as ControlDevice, ResourceInfo};
use gbm::BufferObject;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::os::unix::io::RawFd;

use backend::session::{AsSessionObserver, SessionObserver};
use backend::drm::{Device, RawDevice, RawSurface};
use super::{GbmDevice, GbmSurface};

/// `SessionObserver` linked to the `DrmDevice` it was created from.
pub struct GbmDeviceObserver<
    S: SessionObserver + 'static,
    D: RawDevice + ControlDevice + AsSessionObserver<S> + 'static,
>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>
{
    observer: S,
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<GbmSurface<D>>>>>,
    logger: ::slog::Logger,
}

impl<
    S: SessionObserver + 'static,
    D: RawDevice + ControlDevice + AsSessionObserver<S> + 'static,
> AsSessionObserver<GbmDeviceObserver<S, D>> for GbmDevice<D>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>
{
    fn observer(&mut self) -> GbmDeviceObserver<S, D> {
        GbmDeviceObserver {
            observer: (**self.dev.borrow_mut()).observer(),
            backends: Rc::downgrade(&self.backends),
            logger: self.logger.clone(),
        }
    }
}

impl<
    S: SessionObserver + 'static,
    D: RawDevice + ControlDevice + AsSessionObserver<S> + 'static,
> SessionObserver for GbmDeviceObserver<S, D>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>
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
                    // restart rendering loop
                    if let Err(err) = 
                        ::std::borrow::Borrow::borrow(&backend.crtc).page_flip(backend.current_frame_buffer.get().handle())
                    {
                        warn!(self.logger, "Failed to restart rendering loop. Error: {}", err);
                    }
                    // reset cursor
                    {
                        let &(ref cursor, ref hotspot): &(BufferObject<()>, (u32, u32)) =
                            unsafe { &*backend.cursor.as_ptr() };
                        if crtc::set_cursor2(
                            &*backend.dev.borrow(),
                            *crtc,
                            cursor,
                            ((*hotspot).0 as i32, (*hotspot).1 as i32),
                        ).is_err()
                        {
                            if let Err(err) = crtc::set_cursor(&*backend.dev.borrow(), *crtc, cursor) {
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
