use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Weak,
};

use drm::Device as BasicDevice;
use nix::libc::dev_t;
use nix::sys::stat;

use super::device::{DrmDevice, DrmDeviceInternal};
use super::error::Error;
use crate::{
    backend::session::Signal as SessionSignal,
    signaling::{Linkable, Signaler},
};

/// [`SessionObserver`](SessionObserver)
/// linked to the [`DrmDevice`](DrmDevice)
/// it was created from.
pub struct DrmDeviceObserver<A: AsRawFd + 'static> {
    dev_id: dev_t,
    dev: Weak<DrmDeviceInternal<A>>,
    privileged: bool,
    active: Arc<AtomicBool>,
    logger: ::slog::Logger,
}

impl<A: AsRawFd + 'static> Linkable<SessionSignal> for DrmDevice<A> {
    fn link(&mut self, signaler: Signaler<SessionSignal>) {
        let mut observer = DrmDeviceObserver {
            dev: Arc::downgrade(&self.internal),
            dev_id: self.dev_id,
            active: match &*self.internal {
                DrmDeviceInternal::Atomic(dev) => dev.active.clone(),
                DrmDeviceInternal::Legacy(dev) => dev.active.clone(),
            },
            privileged: match &*self.internal {
                DrmDeviceInternal::Atomic(dev) => dev.fd.privileged,
                DrmDeviceInternal::Legacy(dev) => dev.fd.privileged,
            },
            logger: self.logger.new(o!("drm_module" => "observer")),
        };

        let token = signaler.register(move |signal| observer.signal(*signal));
        self.links.borrow_mut().push(token);
    }
}

impl<A: AsRawFd + 'static> DrmDeviceObserver<A> {
    fn signal(&mut self, signal: SessionSignal) {
        match signal {
            SessionSignal::PauseSession => self.pause(None),
            SessionSignal::PauseDevice { major, minor } => self.pause(Some((major, minor))),
            SessionSignal::ActivateSession => self.activate(None),
            SessionSignal::ActivateDevice { major, minor, new_fd } => {
                self.activate(Some((major, minor, new_fd)))
            }
        }
    }

    fn pause(&mut self, devnum: Option<(u32, u32)>) {
        if let Some((major, minor)) = devnum {
            if major as u64 != stat::major(self.dev_id) || minor as u64 != stat::minor(self.dev_id) {
                return;
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

        // okay, the previous session/whatever might left the drm devices in any state...
        // lets fix that
        if let Err(err) = self.reset_state() {
            warn!(self.logger, "Unable to reset state after tty switch: {}", err);
            // TODO call drm-handler::error
        }

        self.active.store(true, Ordering::SeqCst);
    }

    fn reset_state(&mut self) -> Result<(), Error> {
        if let Some(dev) = self.dev.upgrade() {
            dev.reset_state()
        } else {
            Ok(())
        }
    }
}
