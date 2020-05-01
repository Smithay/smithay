//!
//! Support to register an open [`LegacyDrmDevice`](LegacyDrmDevice)
//! to an open [`Session`](::backend::session::Session).
//!

use drm::control::{crtc, Device as ControlDevice};
use drm::Device as BasicDevice;
use failure::ResultExt;
use nix::libc::dev_t;
use nix::sys::stat;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{Dev, DevPath, Error, LegacyDrmDevice, LegacyDrmSurfaceInternal};
use crate::backend::session::{AsSessionObserver, SessionObserver};

/// [`SessionObserver`](SessionObserver)
/// linked to the [`LegacyDrmDevice`](LegacyDrmDevice)
/// it was created from.
pub struct LegacyDrmDeviceObserver<A: AsRawFd + 'static> {
    dev: Weak<Dev<A>>,
    dev_id: dev_t,
    privileged: bool,
    active: Arc<AtomicBool>,
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<LegacyDrmSurfaceInternal<A>>>>>,
    logger: ::slog::Logger,
}

impl<A: AsRawFd + 'static> AsSessionObserver<LegacyDrmDeviceObserver<A>> for LegacyDrmDevice<A> {
    fn observer(&mut self) -> LegacyDrmDeviceObserver<A> {
        LegacyDrmDeviceObserver {
            dev: Rc::downgrade(&self.dev),
            dev_id: self.dev_id,
            active: self.active.clone(),
            privileged: self.dev.privileged,
            backends: Rc::downgrade(&self.backends),
            logger: self.logger.clone(),
        }
    }
}

impl<A: AsRawFd + 'static> SessionObserver for LegacyDrmDeviceObserver<A> {
    fn pause(&mut self, devnum: Option<(u32, u32)>) {
        if let Some((major, minor)) = devnum {
            if major as u64 != stat::major(self.dev_id) || minor as u64 != stat::minor(self.dev_id) {
                return;
            }
        }
        if let Some(device) = self.dev.upgrade() {
            if let Some(backends) = self.backends.upgrade() {
                for surface in backends.borrow().values().filter_map(Weak::upgrade) {
                    // other ttys that use no cursor, might not clear it themselves.
                    // This makes sure our cursor won't stay visible.
                    let _ = (*device).set_cursor(
                        surface.crtc,
                        Option::<&drm::control::dumbbuffer::DumbBuffer>::None,
                    );
                }
            }
        }
        self.active.store(false, Ordering::SeqCst);
        if self.privileged {
            if let Some(device) = self.dev.upgrade() {
                if let Err(err) = device.release_master_lock() {
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
    }
}

impl<A: AsRawFd + 'static> LegacyDrmDeviceObserver<A> {
    fn reset_state(&mut self) -> Result<(), Error> {
        // lets enumerate it the current state
        if let Some(dev) = self.dev.upgrade() {
            let res_handles = ControlDevice::resource_handles(&*dev)
                .compat()
                .map_err(|source| Error::Access {
                    errmsg: "Error loading drm resources",
                    dev: dev.dev_path(),
                    source,
                })?;

            let mut used_connectors = HashSet::new();
            if let Some(backends) = self.backends.upgrade() {
                for surface in backends.borrow().values().filter_map(Weak::upgrade) {
                    let mut current = surface.state.write().unwrap();
                    let pending = surface.pending.read().unwrap();

                    // store (soon to be) used connectors
                    used_connectors.extend(pending.connectors.clone());

                    // set current connectors
                    current.connectors.clear();
                    for conn in res_handles.connectors() {
                        let conn_info =
                            dev.get_connector(*conn)
                                .compat()
                                .map_err(|source| Error::Access {
                                    errmsg: "Could not load connector info",
                                    dev: dev.dev_path(),
                                    source,
                                })?;
                        if let Some(enc) = conn_info.current_encoder() {
                            let enc_info = dev.get_encoder(enc).compat().map_err(|source| Error::Access {
                                errmsg: "Could not load encoder info",
                                dev: dev.dev_path(),
                                source,
                            })?;
                            if enc_info.crtc().map(|crtc| crtc == surface.crtc).unwrap_or(false) {
                                current.connectors.insert(*conn);
                            }
                        }
                    }

                    // set current mode
                    let crtc_info = dev
                        .get_crtc(surface.crtc)
                        .compat()
                        .map_err(|source| Error::Access {
                            errmsg: "Could not load crtc info",
                            dev: dev.dev_path(),
                            source,
                        })?;

                    // If we have no current mode, we create a fake one, which will not match (and thus gets overriden on the commit below).
                    // A better fix would probably be making mode an `Option`, but that would mean
                    // we need to be sure, we require a mode to always be set without relying on the compiler.
                    // So we cheat, because it works and is easier to handle later.
                    current.mode = crtc_info.mode().unwrap_or_else(|| unsafe { std::mem::zeroed() });
                }
            }

            // Disable unused connectors
            let all_set = res_handles.connectors().iter().copied().collect::<HashSet<_>>();
            let unused = used_connectors.difference(&all_set);
            dev.set_connector_state(unused.copied(), false)?;
        }

        Ok(())
    }
}
