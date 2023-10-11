use drm::{control::Device as ControlDevice, Device as BasicDevice};
use std::{
    os::unix::io::{AsFd, AsRawFd, BorrowedFd, RawFd},
    sync::Arc,
};
use tracing::{error, info, warn};

use crate::utils::{DevPath, DeviceFd};

#[derive(Debug)]
struct InternalDrmDeviceFd {
    fd: DeviceFd,
    privileged: bool,
}

impl Drop for InternalDrmDeviceFd {
    fn drop(&mut self) {
        info!("Dropping device: {:?}", self.fd.dev_path());
        if self.privileged {
            if let Err(err) = self.release_master_lock() {
                error!("Failed to drop drm master state. Error: {}", err);
            }
        }
    }
}

impl AsFd for InternalDrmDeviceFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}
impl BasicDevice for InternalDrmDeviceFd {}
impl ControlDevice for InternalDrmDeviceFd {}

/// Ref-counted file descriptor of an open drm device
#[derive(Debug, Clone)]
pub struct DrmDeviceFd(Arc<InternalDrmDeviceFd>);

impl AsFd for DrmDeviceFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.fd.as_fd()
    }
}

// TODO: drop impl once not needed anymore by smithay or dependencies
impl AsRawFd for DrmDeviceFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0.fd.as_raw_fd()
    }
}

impl DrmDeviceFd {
    /// Create a new `DrmDeviceFd`.
    ///
    /// This function will try to acquire the master lock for the underlying drm device
    /// and release the lock on drop again.
    /// For that reason you should never create multiple `DrmDeviceFd` out of the same
    /// `DeviceFd`, but instead clone the `DrmDeviceFd`.
    ///
    /// Failing to do so might fail to acquire set lock and release it early,
    /// which can cause some drm ioctls to fail later.
    pub fn new(fd: DeviceFd) -> DrmDeviceFd {
        let mut dev = InternalDrmDeviceFd {
            fd,
            privileged: false,
        };

        // We want to modeset, so we better be the master, if we run via a tty session.
        // This is only needed on older kernels. Newer kernels grant this permission,
        // if no other process is already the *master*. So we skip over this error.
        if dev.acquire_master_lock().is_err() {
            warn!("Unable to become drm master, assuming unprivileged mode");
        } else {
            dev.privileged = true;
        }

        DrmDeviceFd(Arc::new(dev))
    }

    pub(in crate::backend::drm) fn is_privileged(&self) -> bool {
        self.0.privileged
    }

    /// Returns the underlying `DeviceFd`
    pub fn device_fd(&self) -> DeviceFd {
        self.0.fd.clone()
    }

    /// Returns the `dev_t` of the underlying device
    pub fn dev_id(&self) -> rustix::io::Result<libc::dev_t> {
        Ok(rustix::fs::fstat(&self.0.fd)?.st_rdev)
    }
}

impl BasicDevice for DrmDeviceFd {}
impl ControlDevice for DrmDeviceFd {}
