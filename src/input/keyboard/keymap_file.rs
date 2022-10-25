use crate::utils::sealed_file::SealedFile;
use slog::error;
use xkbcommon::xkb::{Keymap, KEYMAP_FORMAT_TEXT_V1};

use std::ffi::CString;
#[cfg(feature = "wayland_frontend")]
use std::os::unix::prelude::RawFd;

/// Wraps an XKB keymap into a sealed file or stores as just a string for sending to WlKeyboard over an fd
#[cfg(feature = "wayland_frontend")]
#[derive(Debug)]
pub struct KeymapFile {
    sealed: Option<SealedFile>,
    keymap: String,
}

impl KeymapFile {
    /// Turn the keymap into a string using KEYMAP_FORMAT_TEXT_V1, create a sealed file for it, and store the string
    pub fn new<L>(keymap: &Keymap, logger: L) -> Self
    where
        L: Into<Option<slog::Logger>>,
    {
        let logger = crate::slog_or_fallback(logger);
        let name = CString::new("smithay-keymap").unwrap();
        let keymap = keymap.get_as_string(KEYMAP_FORMAT_TEXT_V1);
        let sealed = SealedFile::new(name, CString::new(keymap.as_str()).unwrap());

        if let Err(err) = sealed.as_ref() {
            error!(logger, "Error when creating sealed keymap file: {}", err);
        }

        Self {
            sealed: sealed.ok(),
            keymap,
        }
    }

    /// Run a closure with the file descriptor to ensure safety
    pub fn with_fd<F>(&self, supports_sealed: bool, cb: F) -> Result<(), std::io::Error>
    where
        F: FnOnce(RawFd, usize),
    {
        use std::{io::Write, os::unix::prelude::AsRawFd, path::PathBuf};

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

    /// Send the keymap contained within to a WlKeyboard
    pub fn send(
        &self,
        keyboard: &wayland_server::protocol::wl_keyboard::WlKeyboard,
    ) -> Result<(), std::io::Error> {
        use wayland_server::{protocol::wl_keyboard::KeymapFormat, Resource};

        self.with_fd(keyboard.version() >= 7, |fd, size| {
            keyboard.keymap(KeymapFormat::XkbV1, fd, size as u32);
        })
    }
}
