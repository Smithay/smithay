//!
//! Support to register an open [`AtomicDrmDevice`](AtomicDrmDevice)
//! to an open [`Session`](::backend::session::Session).
//!

use drm::control::crtc;
use drm::Device as BasicDevice;
use nix::libc::dev_t;
use nix::sys::stat;
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{AtomicDrmDevice, AtomicDrmSurfaceInternal, Dev};
use crate::backend::session::{AsSessionObserver, SessionObserver};

/// [`SessionObserver`](SessionObserver)
/// linked to the [`AtomicDrmDevice`](AtomicDrmDevice)
/// it was created from.
pub struct AtomicDrmDeviceObserver<A: AsRawFd + 'static> {
    dev: Weak<Dev<A>>,
    dev_id: dev_t,
    privileged: bool,
    active: Arc<AtomicBool>,
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<AtomicDrmSurfaceInternal<A>>>>>,
    logger: ::slog::Logger,
}

impl<A: AsRawFd + 'static> AsSessionObserver<AtomicDrmDeviceObserver<A>> for AtomicDrmDevice<A> {
    fn observer(&mut self) -> AtomicDrmDeviceObserver<A> {
        AtomicDrmDeviceObserver {
            dev: Rc::downgrade(&self.dev),
            dev_id: self.dev_id,
            active: self.active.clone(),
            privileged: self.dev.privileged,
            backends: Rc::downgrade(&self.backends),
            logger: self.logger.clone(),
        }
    }
}

impl<A: AsRawFd + 'static> SessionObserver for AtomicDrmDeviceObserver<A> {
    fn pause(&mut self, devnum: Option<(u32, u32)>) {
        if let Some((major, minor)) = devnum {
            if major as u64 != stat::major(self.dev_id) || minor as u64 != stat::minor(self.dev_id) {
                return;
            }
        }
        // TODO: Clear overlay planes (if we ever use them)
        if let Some(backends) = self.backends.upgrade() {
            for surface in backends.borrow().values().filter_map(Weak::upgrade) {
                // other ttys that use no cursor, might not clear it themselves.
                // This makes sure our cursor won't stay visible.
                if let Err(err) = surface.clear_plane(surface.planes.cursor) {
                    warn!(
                        self.logger,
                        "Failed to clear cursor on {:?}: {}", surface.crtc, err
                    );
                }
            }
        }

        self.active.store(false, Ordering::SeqCst);
        if self.privileged {
            if let Some(device) = self.dev.upgrade() {
                if let Err(err) = device.release_master_lock() {
                    error!(self.logger, "Failed to drop drm master state Error: {}", err);
                }
            }
        }
    }

    fn activate(&mut self, devnum: Option<(u32, u32, Option<RawFd>)>) {
        if let Some((major, minor, fd)) = devnum {
            if major as u64 != stat::major(self.dev_id) || minor as u64 != stat::minor(self.dev_id) {
                return;
            } else if let Some(fd) = fd {
                info!(self.logger, "Replacing fd");
                if let Some(device) = self.dev.upgrade() {
                    ::nix::unistd::dup2(device.as_raw_fd(), fd)
                        .expect("Failed to replace file descriptor of drm device");
                }
            }
        }
        if self.privileged {
            if let Some(device) = self.dev.upgrade() {
                if let Err(err) = device.acquire_master_lock() {
                    crit!(self.logger, "Failed to acquire drm master again. Error: {}", err);
                }
            }
        }
        self.active.store(true, Ordering::SeqCst);
    }
}
