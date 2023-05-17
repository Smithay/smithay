//! A file whose fd cannot be written by other processes
//!
//! This mechanism is useful for giving clients access to large amounts of
//! information such as keymaps without them being able to write to the handle.

use std::{
    ffi::CString,
    fs::File,
    io::Write,
    os::unix::io::{AsRawFd, FromRawFd, RawFd},
};

#[derive(Debug)]
pub(crate) struct SealedFile {
    file: File,
    size: usize,
}

impl SealedFile {
    pub fn with_content(name: CString, contents: CString) -> Result<Self, std::io::Error> {
        Self::with_data(name, contents.as_bytes_with_nul())
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "android"))]
    pub fn with_data(name: CString, data: &[u8]) -> Result<Self, std::io::Error> {
        use nix::{
            fcntl::{FcntlArg, SealFlag},
            sys::memfd::MemFdCreateFlag,
        };
        use std::io::Seek;

        let fd = nix::sys::memfd::memfd_create(
            &name,
            MemFdCreateFlag::MFD_CLOEXEC | MemFdCreateFlag::MFD_ALLOW_SEALING,
        )?;

        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(data)?;
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
            size: data.len(),
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "android")))]
    pub fn with_data(name: CString, data: &[u8]) -> Result<Self, std::io::Error> {
        use nix::{
            errno::Errno,
            fcntl::OFlag,
            sys::{mman, stat::Mode},
        };
        use rand::{distributions::Alphanumeric, Rng};

        let mut rng = rand::thread_rng();

        // `memfd_create` isn't available. Instead, try `shm_open` with a randomized name, and
        // loop a couple times if it exists.
        let mut n = 0;
        let (shm_name, mut file) = loop {
            let mut shm_name = name.as_bytes().to_owned();
            shm_name.push(b'-');
            shm_name.extend((0..7).map(|_| rng.sample(Alphanumeric)));
            let fd = mman::shm_open(
                shm_name.as_slice(),
                OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_EXCL,
                Mode::S_IRWXU,
            );
            if fd != Err(Errno::EEXIST) || n > 3 {
                break (shm_name, unsafe { File::from_raw_fd(fd?) });
            }
            n += 1;
        };

        // Sealing isn't available, so re-open read-only.
        let fd_rdonly = mman::shm_open(shm_name.as_slice(), OFlag::O_RDONLY, Mode::empty())?;
        let file_rdonly = unsafe { File::from_raw_fd(fd_rdonly) };

        // Unlink so another process can't open shm file.
        let _ = mman::shm_unlink(shm_name.as_slice());

        file.write_all(data)?;
        file.flush()?;

        Ok(Self {
            file: file_rdonly,
            size: data.len(),
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
