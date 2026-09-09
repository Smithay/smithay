use std::ffi::CString;
use std::os::unix::io::{AsFd, BorrowedFd};

use tracing::error;
use xkbcommon::xkb::{self, KEYMAP_FORMAT_TEXT_V1, Keymap};

use crate::utils::SealedFile;

/// Unique ID for a keymap
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct KeymapFileId([u8; 32]);

// Generates a 32-byte (256-bit) hash using `std::hash::DefaultHasher`.
// This is sufficient to prevent keymap collisions, but it is not cryptographically
// secure or guaranteed to be stable across different Rust versions.
fn hash(data: &[u8]) -> [u8; 32] {
    use std::hash::{DefaultHasher, Hasher};
    let mut output = [0u8; 32];
    let mut prev_hash: Option<[u8; 8]> = None;

    for i in (0..32).step_by(8) {
        let mut hasher = DefaultHasher::new();
        if let Some(prev_hash_val) = prev_hash {
            hasher.write(&prev_hash_val);
        }
        hasher.write(data);
        let hash = hasher.finish().to_le_bytes();
        output[i..i + 8].copy_from_slice(&hash);
        prev_hash = Some(hash);
    }

    output
}

impl KeymapFileId {
    fn for_keymap(keymap: &str) -> Self {
        // Use a hash to avoid sending redundant `keymap` events when the keymap has not changed,
        // which is particularly useful for `virtual-keyboard-unstable-v1`.
        Self(hash(keymap.as_bytes()))
    }
}

/// Wraps an XKB keymap into a sealed file or stores as just a string for sending to WlKeyboard over an fd
#[derive(Debug)]
pub struct KeymapFile {
    sealed: Option<SealedFile>,
    keymap: String,
    id: KeymapFileId,
}

impl KeymapFile {
    /// Turn the keymap into a string using KEYMAP_FORMAT_TEXT_V1, create a sealed file for it, and store the string
    pub fn new(keymap: &Keymap) -> Self {
        let name = c"smithay-keymap";
        let keymap = keymap.get_as_string(KEYMAP_FORMAT_TEXT_V1);
        let sealed = SealedFile::with_content(name, &CString::new(keymap.as_str()).unwrap());

        if let Err(err) = sealed.as_ref() {
            error!("Error when creating sealed keymap file: {}", err);
        }

        Self {
            id: KeymapFileId::for_keymap(&keymap),
            sealed: sealed.ok(),
            keymap,
        }
    }

    #[cfg(feature = "wayland_frontend")]
    pub(crate) fn change_keymap(&mut self, keymap: &Keymap) {
        let keymap = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);

        let name = c"smithay-keymap-file";
        let sealed = SealedFile::with_content(name, &CString::new(keymap.clone()).unwrap());

        if let Err(err) = sealed.as_ref() {
            error!("Error when creating sealed keymap file: {}", err);
        }

        self.id = KeymapFileId::for_keymap(&keymap);
        self.sealed = sealed.ok();
        self.keymap = keymap;
    }

    #[cfg(feature = "wayland_frontend")]
    /// Run a closure with the file descriptor to ensure safety
    pub fn with_fd<F>(&self, supports_sealed: bool, cb: F) -> Result<(), std::io::Error>
    where
        F: FnOnce(BorrowedFd<'_>, usize),
    {
        use std::{io::Write, path::PathBuf};

        if let Some(file) = supports_sealed.then_some(self.sealed.as_ref()).flatten() {
            cb(file.as_fd(), file.size());
        } else {
            let dir = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);

            let mut file = tempfile::tempfile_in(dir)?;
            file.write_all(self.keymap.as_bytes())?;
            file.flush()?;

            cb(file.as_fd(), self.keymap.len());
        }
        Ok(())
    }

    /// Send the keymap contained within to a WlKeyboard
    pub fn send(
        &self,
        keyboard: &wayland_server::protocol::wl_keyboard::WlKeyboard,
    ) -> Result<(), std::io::Error> {
        use wayland_server::{Resource, protocol::wl_keyboard::KeymapFormat};

        self.with_fd(keyboard.version() >= 7, |fd, size| {
            keyboard.keymap(KeymapFormat::XkbV1, fd, size as u32);
        })
    }

    /// Get this keymap's unique ID.
    pub(crate) fn id(&self) -> KeymapFileId {
        self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xkbcommon::xkb;

    #[test]
    fn test_keymap_file_id() {
        let id1 = KeymapFileId::for_keymap("keymap data 1");
        let id2 = KeymapFileId::for_keymap("keymap data 2");
        let id3 = KeymapFileId::for_keymap("keymap data 1");

        assert_ne!(id1, id2);
        assert_eq!(id1, id3);
    }

    #[test]
    fn test_keymap_file_creation() {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let xkb_config = crate::input::keyboard::XkbConfig::default();
        let keymap = xkb_config.compile_keymap(&context).unwrap();
        let keymap_file = KeymapFile::new(&keymap);

        assert_ne!(keymap_file.id().0, [0u8; 32]);
    }
}
