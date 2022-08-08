use std::{
    ffi::{CStr, CString},
    fs::File,
    io::{Seek, Write},
    os::unix::prelude::{AsRawFd, FromRawFd, RawFd},
    path::PathBuf,
};

use nix::{
    fcntl::{FcntlArg, SealFlag},
    sys::memfd::MemFdCreateFlag,
};
use slog::error;

#[derive(Debug)]
pub struct KeymapFile {
    #[allow(dead_code)]
    sealed: Option<SealedFile>,
    keymap: CString,
}

impl KeymapFile {
    pub fn new(keymap: CString, log: slog::Logger) -> Self {
        let sealed = SealedFile::new(&keymap);

        if let Err(err) = sealed.as_ref() {
            error!(log, "Error when creating sealed keymap file: {}", err);
        }

        Self {
            sealed: sealed.ok(),
            keymap,
        }
    }

    #[cfg(feature = "wayland_frontend")]
    pub fn with_fd<F>(&self, supports_sealed: bool, cb: F) -> Result<(), std::io::Error>
    where
        F: FnOnce(RawFd, usize),
    {
        if let Some(file) = supports_sealed.then(|| self.sealed.as_ref()).flatten() {
            cb(file.as_raw_fd(), file.size);
            Ok(())
        } else {
            let dir = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);

            let keymap = self.keymap.as_bytes_with_nul();
            let mut file = tempfile::tempfile_in(dir)?;
            file.write_all(keymap)?;
            file.flush()?;

            cb(file.as_raw_fd(), keymap.len());
            Ok(())
        }
    }
}

#[derive(Debug)]
struct SealedFile {
    file: File,
    #[allow(dead_code)]
    size: usize,
}

impl SealedFile {
    fn new(keymap: &CStr) -> Result<Self, std::io::Error> {
        let name = CString::new("smithay-keymap").expect("File name should not contain interior nul byte");
        let keymap = keymap.to_bytes_with_nul();

        let fd = nix::sys::memfd::memfd_create(
            &name,
            MemFdCreateFlag::MFD_CLOEXEC | MemFdCreateFlag::MFD_ALLOW_SEALING,
        )?;

        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(keymap)?;
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
            size: keymap.len(),
        })
    }
}

impl AsRawFd for SealedFile {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}
