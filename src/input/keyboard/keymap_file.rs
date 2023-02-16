use crate::utils::sealed_file::SealedFile;
use tracing::error;
use xkbcommon::xkb::{Keymap, KEYMAP_FORMAT_TEXT_V1};

use std::ffi::CString;
use std::os::unix::io::RawFd;

/// Wraps an XKB keymap into a sealed file or stores as just a string for sending to WlKeyboard over an fd
#[derive(Debug)]
pub struct KeymapFile {
    sealed: Option<SealedFile>,
    keymap: String,
}

impl KeymapFile {
    /// Turn the keymap into a string using KEYMAP_FORMAT_TEXT_V1, create a sealed file for it, and store the string
    pub fn new(keymap: &Keymap) -> Self {
        let name = CString::new("smithay-keymap").unwrap();
        let keymap = keymap.get_as_string(KEYMAP_FORMAT_TEXT_V1);
        let sealed = SealedFile::new(name, CString::new(keymap.as_str()).unwrap());

        if let Err(err) = sealed.as_ref() {
            error!("Error when creating sealed keymap file: {}", err);
        }

        Self {
            sealed: sealed.ok(),
            keymap,
        }
    }

    #[cfg(feature = "wayland_frontend")]
    pub(crate) fn change_keymap(&mut self, keymap: String) {
        let name = CString::new("smithay-keymap-file").unwrap();
        let sealed = SealedFile::new(name, CString::new(keymap.clone()).unwrap());

        if let Err(err) = sealed.as_ref() {
            error!("Error when creating sealed keymap file: {}", err);
        }

        self.sealed = sealed.ok();
        self.keymap = keymap;
    }

    #[cfg(feature = "wayland_frontend")]
    /// Run a closure with the file descriptor to ensure safety
    pub fn with_fd<F>(&self, supports_sealed: bool, cb: F) -> Result<(), std::io::Error>
    where
        F: FnOnce(RawFd, usize),
    {
        use std::{io::Write, os::unix::io::AsRawFd, path::PathBuf};

        if let Some(file) = supports_sealed.then_some(self.sealed.as_ref()).flatten() {
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
