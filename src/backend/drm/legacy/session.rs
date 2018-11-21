use drm::control::{connector, crtc, Device as ControlDevice};
use drm::Device as BasicDevice;
use nix::libc::dev_t;
use nix::sys::stat;
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{Dev, LegacyDrmDevice, LegacyDrmSurface};
use backend::session::{AsSessionObserver, SessionObserver};

/// `SessionObserver` linked to the `DrmDevice` it was created from.
pub struct LegacyDrmDeviceObserver<A: AsRawFd + 'static> {
    dev: Weak<Dev<A>>,
    dev_id: dev_t,
    priviledged: bool,
    active: Arc<AtomicBool>,
    old_state: HashMap<crtc::Handle, (crtc::Info, Vec<connector::Handle>)>,
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<LegacyDrmSurface<A>>>>>,
    logger: ::slog::Logger,
}

impl<A: ControlDevice + 'static> AsSessionObserver<LegacyDrmDeviceObserver<A>> for LegacyDrmDevice<A> {
    fn observer(&mut self) -> LegacyDrmDeviceObserver<A> {
        LegacyDrmDeviceObserver {
            dev: Rc::downgrade(&self.dev),
            dev_id: self.dev_id,
            old_state: self.old_state.clone(),
            active: self.active.clone(),
            priviledged: self.priviledged,
            backends: Rc::downgrade(&self.backends),
            logger: self.logger.clone(),
        }
    }
}

impl<A: ControlDevice + 'static> SessionObserver for LegacyDrmDeviceObserver<A> {
    fn pause(&mut self, devnum: Option<(u32, u32)>) {
        if let Some((major, minor)) = devnum {
            if major as u64 != stat::major(self.dev_id) || minor as u64 != stat::minor(self.dev_id) {
                return;
            }
        }
        if let Some(device) = self.dev.upgrade() {
            if let Some(backends) = self.backends.upgrade() {
                for surface in backends.borrow().values().filter_map(Weak::upgrade) {
                    let _ = crtc::clear_cursor(&*device, surface.crtc);
                }
            }
            for (handle, &(ref info, ref connectors)) in &self.old_state {
                if let Err(err) = crtc::set(
                    &*device,
                    *handle,
                    info.fb(),
                    connectors,
                    info.position(),
                    info.mode(),
                ) {
                    error!(self.logger, "Failed to reset crtc ({:?}). Error: {}", handle, err);
                }
            }
        }
        self.active.store(false, Ordering::SeqCst);
        if self.priviledged {
            if let Some(device) = self.dev.upgrade() {
                if let Err(err) = device.drop_master() {
                    error!(self.logger, "Failed to drop drm master state. Error: {}", err);
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
        self.active.store(true, Ordering::SeqCst);
        if self.priviledged {
            if let Some(device) = self.dev.upgrade() {
                if let Err(err) = device.set_master() {
                    crit!(self.logger, "Failed to acquire drm master again. Error: {}", err);
                }
            }
        }
    }
}
