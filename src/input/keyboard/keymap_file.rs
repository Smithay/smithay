use crate::utils::sealed_file::SealedFile;
use slog::error;
use std::{
    io::Write,
    os::unix::prelude::{AsRawFd, RawFd},
    path::PathBuf,
};
use xkbcommon::xkb::{Keymap, KEYMAP_FORMAT_TEXT_V1};

#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::wl_keyboard::WlKeyboard;

#[derive(Debug)]
#[allow(dead_code)]
pub struct KeymapFile {
    sealed: Option<SealedFile>,
    keymap: String,
}

impl KeymapFile {
    pub fn new(keymap: Keymap, log: slog::Logger) -> Self {
        let keymap = keymap.get_as_string(KEYMAP_FORMAT_TEXT_V1);
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
            cb(file.as_raw_fd(), file.size());
        } else {
            let dir = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);

            let mut file = tempfile::tempfile_in(dir)?;
            file.write_all(self.keymap.as_bytes())?;
            file.flush()?;

            cb(file.as_raw_fd(), self.keymap.len());
        }
        Ok(())
    }

    #[cfg(feature = "wayland_frontend")]
    pub fn send(&self, keyboard: &WlKeyboard) -> Result<(), std::io::Error> {
        use wayland_server::{protocol::wl_keyboard::KeymapFormat, Resource};

        self.with_fd(keyboard.version() >= 7, |fd, size| {
            keyboard.keymap(KeymapFormat::XkbV1, fd, size as u32);
        })
    }
}
