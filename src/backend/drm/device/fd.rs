use drm::{control::Device as ControlDevice, Device as BasicDevice};
use std::{
    os::unix::io::{AsFd, AsRawFd, BorrowedFd, RawFd},
    sync::Arc,
};

use crate::utils::{DevPath, DeviceFd};

#[derive(Debug)]
struct InternalDrmDeviceFd {
    fd: DeviceFd,
    privileged: bool,
    logger: slog::Logger,
}

impl Drop for InternalDrmDeviceFd {
    fn drop(&mut self) {
        slog::info!(self.logger, "Dropping device: {:?}", self.fd.dev_path());
        if self.privileged {
            if let Err(err) = self.release_master_lock() {
                slog::error!(self.logger, "Failed to drop drm master state. Error: {}", err);
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
    pub fn new(fd: DeviceFd, logger: impl Into<Option<slog::Logger>>) -> DrmDeviceFd {
        let mut dev = InternalDrmDeviceFd {
            fd,
            privileged: false,
            logger: crate::slog_or_fallback(logger).new(slog::o!("smithay_module" => "backend_drm")),
        };

        // We want to modeset, so we better be the master, if we run via a tty session.
        // This is only needed on older kernels. Newer kernels grant this permission,
        // if no other process is already the *master*. So we skip over this error.
        if dev.acquire_master_lock().is_err() {
            slog::warn!(
                dev.logger,
                "Unable to become drm master, assuming unprivileged mode"
            );
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
    pub fn dev_id(&self) -> Result<nix::libc::dev_t, nix::Error> {
        Ok(nix::sys::stat::fstat(self.0.fd.as_raw_fd())?.st_rdev)
    }
}

impl BasicDevice for DrmDeviceFd {}
impl ControlDevice for DrmDeviceFd {}
