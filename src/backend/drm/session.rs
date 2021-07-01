use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Weak,
};

use drm::{
    control::{crtc, Device as ControlDevice},
    Device as BasicDevice,
};
use nix::libc::dev_t;
use nix::sys::stat;

use super::device::{DrmDevice, DrmDeviceInternal};
use super::surface::{DrmSurface, DrmSurfaceInternal};
use crate::{
    backend::session::Signal as SessionSignal,
    utils::signaling::{Linkable, Signaler},
};

use slog::{crit, error, info, o, warn};

struct DrmDeviceObserver<A: AsRawFd + 'static> {
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

        self.active.store(true, Ordering::SeqCst);
    }
}

struct DrmSurfaceObserver<A: AsRawFd + 'static> {
    dev_id: dev_t,
    crtc: crtc::Handle,
    surf: Weak<DrmSurfaceInternal<A>>,
    logger: ::slog::Logger,
}

struct FdHack(RawFd);
impl AsRawFd for FdHack {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}
impl BasicDevice for FdHack {}
impl ControlDevice for FdHack {}

impl<A: AsRawFd + 'static> Linkable<SessionSignal> for DrmSurface<A> {
    fn link(&mut self, signaler: Signaler<SessionSignal>) {
        let logger = match &*self.internal {
            DrmSurfaceInternal::Atomic(surf) => surf.logger.clone(),
            DrmSurfaceInternal::Legacy(surf) => surf.logger.clone(),
        };
        let mut observer = DrmSurfaceObserver {
            dev_id: self.dev_id,
            crtc: self.crtc(),
            surf: Arc::downgrade(&self.internal),
            logger: logger.new(o!("drm_module" => "observer")),
        };

        let token = signaler.register(move |signal| observer.signal(*signal));
        self.links.borrow_mut().push(token);
    }
}

impl<A: AsRawFd + 'static> DrmSurfaceObserver<A> {
    fn signal(&mut self, signal: SessionSignal) {
        match signal {
            SessionSignal::ActivateSession => self.activate(None),
            SessionSignal::ActivateDevice { major, minor, new_fd } => {
                self.activate(Some((major, minor, new_fd)))
            }
            _ => {}
        }
    }

    fn activate(&mut self, devnum: Option<(u32, u32, Option<RawFd>)>) {
        if let Some(surf) = self.surf.upgrade() {
            // The device will reset the _fd, but the observer order is not deterministic,
            // so we might need to use it anyway.
            let fd = if let Some((major, minor, fd)) = devnum {
                if major as u64 != stat::major(self.dev_id) || minor as u64 != stat::minor(self.dev_id) {
                    return;
                }
                fd.map(FdHack)
            } else {
                None
            };

            if let Err(err) = match &*surf {
                DrmSurfaceInternal::Atomic(surf) => surf.reset_state(fd.as_ref()),
                DrmSurfaceInternal::Legacy(surf) => surf.reset_state(fd.as_ref()),
            } {
                warn!(
                    self.logger,
                    "Failed to reset state of surface ({:?}/{:?}): {}", self.dev_id, self.crtc, err
                );
            }
        }
    }
}
