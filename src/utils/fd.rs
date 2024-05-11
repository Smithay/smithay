#![forbid(unsafe_op_in_unsafe_fn)]

use std::{
    os::unix::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
    path::PathBuf,
    sync::Arc,
};

/// Ref-counted file descriptor of an open device node
#[derive(Debug, Clone)]
pub struct DeviceFd(Arc<OwnedFd>);

impl PartialEq for DeviceFd {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0.as_raw_fd() == other.0.as_raw_fd()
    }
}

impl AsFd for DeviceFd {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

// TODO: drop impl once not needed anymore by smithay or dependencies
impl AsRawFd for DeviceFd {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl FromRawFd for DeviceFd {
    /// SAFETY:
    /// Make sure that `fd` is a valid value!
    #[inline]
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        DeviceFd(Arc::new(unsafe { OwnedFd::from_raw_fd(fd) }))
    }
}

impl From<OwnedFd> for DeviceFd {
    #[inline]
    fn from(fd: OwnedFd) -> Self {
        DeviceFd(Arc::new(fd))
    }
}

impl TryInto<OwnedFd> for DeviceFd {
    type Error = DeviceFd;

    #[inline]
    fn try_into(self) -> Result<OwnedFd, Self::Error> {
        Arc::try_unwrap(self.0).map_err(DeviceFd)
    }
}

/// Trait representing open devices that *may* return a `Path`
pub trait DevPath {
    /// Returns the path of the open device if possible
    fn dev_path(&self) -> Option<PathBuf>;
}

impl<A: AsFd> DevPath for A {
    fn dev_path(&self) -> Option<PathBuf> {
        use std::fs;

        fs::read_link(format!("/proc/self/fd/{:?}", self.as_fd().as_raw_fd())).ok()
    }
}
