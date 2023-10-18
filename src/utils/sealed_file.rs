//! A file whose fd cannot be written by other processes
//!
//! This mechanism is useful for giving clients access to large amounts of
//! information such as keymaps without them being able to write to the handle.

use std::{
    ffi::CString,
    fs::File,
    io::Write,
    os::unix::io::{AsFd, AsRawFd, BorrowedFd, RawFd},
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
        use rustix::fs::{MemfdFlags, SealFlags};
        use std::io::Seek;

        let fd = rustix::fs::memfd_create(&name, MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)?;

        let mut file: File = fd.into();
        file.write_all(data)?;
        file.flush()?;

        file.seek(std::io::SeekFrom::Start(0))?;

        rustix::fs::fcntl_add_seals(
            &file,
            SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE,
        )?;

        Ok(Self {
            file,
            size: data.len(),
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "android")))]
    pub fn with_data(name: CString, data: &[u8]) -> Result<Self, std::io::Error> {
        use rand::{distributions::Alphanumeric, Rng};
        use rustix::{
            io::Errno,
            shm::{Mode, ShmOFlags},
        };

        let mut rng = rand::thread_rng();

        // `memfd_create` isn't available. Instead, try `shm_open` with a randomized name, and
        // loop a couple times if it exists.
        let mut n = 0;
        let (shm_name, mut file) = loop {
            let mut shm_name = name.as_bytes().to_owned();
            shm_name.push(b'-');
            shm_name.extend((0..7).map(|_| rng.sample(Alphanumeric)));
            let fd = rustix::shm::shm_open(
                shm_name.as_slice(),
                ShmOFlags::RDWR | ShmOFlags::CREATE | ShmOFlags::EXCL,
                Mode::RWXU,
            );
            if !matches!(fd, Err(Errno::EXIST)) || n > 3 {
                break (shm_name, File::from(fd?));
            }
            n += 1;
        };

        // Sealing isn't available, so re-open read-only.
        let fd_rdonly = rustix::shm::shm_open(shm_name.as_slice(), ShmOFlags::RDONLY, Mode::empty())?;
        let file_rdonly = File::from(fd_rdonly);

        // Unlink so another process can't open shm file.
        let _ = rustix::shm::shm_unlink(shm_name.as_slice());

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

impl AsFd for SealedFile {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}
