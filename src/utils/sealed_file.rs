//! A file whose fd cannot be written by other processes
//!
//! This mechanism is useful for giving clients access to large amounts of
//! information such as keymaps without them being able to write to the handle.

use nix::{
    fcntl::{FcntlArg, SealFlag},
    sys::memfd::MemFdCreateFlag,
};
use std::{
    ffi::CString,
    fs::File,
    io::{Seek, Write},
    os::unix::prelude::{AsRawFd, FromRawFd, RawFd},
};

#[derive(Debug)]
pub(crate) struct SealedFile {
    file: File,
    size: usize,
}

impl SealedFile {
    pub fn new(name: CString, contents: CString) -> Result<Self, std::io::Error> {
        let contents = contents.as_bytes_with_nul();

        let fd = nix::sys::memfd::memfd_create(
            &name,
            MemFdCreateFlag::MFD_CLOEXEC | MemFdCreateFlag::MFD_ALLOW_SEALING,
        )?;

        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(contents)?;
        file.flush()?;

        file.seek(std::io::SeekFrom::Start(0))?;

        nix::fcntl::fcntl(
            file.as_raw_fd(),
            FcntlArg::F_ADD_SEALS(
                SealFlag::F_SEAL_SEAL
                    | SealFlag::F_SEAL_SHRINK
                    | SealFlag::F_SEAL_GROW
                    | SealFlag::F_SEAL_WRITE,
            ),
        )?;

        Ok(Self {
            file,
            size: contents.len(),
        })
    }

    // Only used in KeymapFile which is under the wayland_frontend feature
    pub fn size(&self) -> usize {
        self.size
    }
}

impl AsRawFd for SealedFile {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}
