use drm::{Device as BasicDevice, control::Device as ControlDevice};
use std::{
    os::unix::io::{AsFd, AsRawFd, BorrowedFd, RawFd},
    sync::{Arc, Weak},
    time::Duration,
};
use tracing::{debug, error, info};

/// Number of attempts to acquire the drm master lock on device creation.
///
/// Failing to acquire the lock is *not* an error in the common case: on a libseat/logind session
/// the compositor is handed an already-usable fd and `drmSetMaster` is expected to be refused, and
/// on newer kernels master is granted implicitly on open when nobody else holds it. The only case
/// a retry helps is a genuine transient race - e.g. a crash-respawn where the previous holder's fd
/// has not been reaped yet.
///
/// Keep this budget small: `DrmDeviceFd::new` runs on the compositor's main thread and every
/// attempt sleeps, so the whole budget is added latency on *every* device open, including
/// compositor startup and post-GPU-reset device reopen - i.e. it directly lengthens the black
/// screen the user sees. A previous 10-attempt/~2.3s budget spent that time on every single start
/// while never once succeeding, because the failure was the benign logind case, not a race.
const MASTER_LOCK_ACQUIRE_ATTEMPTS: u32 = 3;
/// Base delay between drm master lock acquisition attempts, doubled on each retry up to
/// `MASTER_LOCK_ACQUIRE_MAX_DELAY`.
const MASTER_LOCK_ACQUIRE_BASE_DELAY: Duration = Duration::from_millis(25);
/// Cap on the per-attempt delay above, so late attempts don't wait longer than necessary.
const MASTER_LOCK_ACQUIRE_MAX_DELAY: Duration = Duration::from_millis(100);

use crate::utils::{DevPath, DeviceFd};

#[derive(Debug)]
struct InternalDrmDeviceFd {
    fd: DeviceFd,
    privileged: bool,
}

impl PartialEq for InternalDrmDeviceFd {
    fn eq(&self, other: &Self) -> bool {
        self.fd == other.fd
    }
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
#[derive(Debug, Clone, PartialEq)]
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
        //
        // Retry a few times with backoff: right after a crash-respawn, the previous holder's
        // master lock may not be released by the kernel yet, and this is otherwise a one-shot
        // check with no other retry path (see MASTER_LOCK_ACQUIRE_ATTEMPTS docs).
        let mut delay = MASTER_LOCK_ACQUIRE_BASE_DELAY;
        for attempt in 1..=MASTER_LOCK_ACQUIRE_ATTEMPTS {
            if dev.acquire_master_lock().is_ok() {
                dev.privileged = true;
                break;
            }
            if attempt < MASTER_LOCK_ACQUIRE_ATTEMPTS {
                debug!(
                    "Unable to become drm master (attempt {}/{}), retrying in {:?}",
                    attempt, MASTER_LOCK_ACQUIRE_ATTEMPTS, delay
                );
                std::thread::sleep(delay);
                delay = std::cmp::min(delay * 2, MASTER_LOCK_ACQUIRE_MAX_DELAY);
            } else {
                // Expected on libseat/logind sessions; not an error on its own.
                info!("Unable to become drm master, assuming unprivileged mode");
            }
        }

        DrmDeviceFd(Arc::new(dev))
    }

    /// Whether this device fd currently holds the DRM master lock.
    ///
    /// A non-privileged fd on what is meant to be the primary/control device for modesetting
    /// cannot page-flip or change the display configuration - callers that require modesetting
    /// (e.g. reopening the primary device after a GPU reset) should treat `false` here as a
    /// failure rather than silently continuing, since [`DrmDeviceFd::new`] itself only logs a
    /// warning and does not fail when master acquisition is exhausted.
    pub fn is_privileged(&self) -> bool {
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

    /// Returns a weak reference to the underlying device
    pub fn downgrade(&self) -> WeakDrmDeviceFd {
        WeakDrmDeviceFd(Arc::downgrade(&self.0))
    }
}

impl BasicDevice for DrmDeviceFd {}
impl ControlDevice for DrmDeviceFd {}

/// Weak variant of [`DrmDeviceFd`]
#[derive(Debug, Clone, Default)]
pub struct WeakDrmDeviceFd(Weak<InternalDrmDeviceFd>);

impl WeakDrmDeviceFd {
    /// Construct an empty Weak reference, that will never upgrade successfully
    pub fn new() -> Self {
        WeakDrmDeviceFd(Weak::new())
    }

    /// Try to upgrade to a strong reference
    pub fn upgrade(&self) -> Option<DrmDeviceFd> {
        self.0.upgrade().map(DrmDeviceFd)
    }
}

impl PartialEq for WeakDrmDeviceFd {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.0, &other.0)
    }
}

impl PartialEq<DrmDeviceFd> for WeakDrmDeviceFd {
    fn eq(&self, other: &DrmDeviceFd) -> bool {
        Weak::upgrade(&self.0).is_some_and(|arc| Arc::ptr_eq(&arc, &other.0))
    }
}
