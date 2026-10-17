use std::ffi::CString;
use std::os::unix::io::{AsFd, BorrowedFd};
use std::sync::Arc;

use siphasher::sip128::SipHasher;
use tracing::error;
use xkbcommon::xkb::{KEYMAP_FORMAT_TEXT_V1, Keymap};

use crate::utils::SealedFile;

/// Unique ID for a keymap
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeymapFileId([u8; 16]);

impl KeymapFileId {
    fn for_keymap(keymap: &str) -> Self {
        // Use a hash, so `keymap` events aren't sent when keymap hasn't changed, particularly
        // with `virtual-keyboard-unstable-v1`.
        Self(SipHasher::new().hash(keymap.as_bytes()).as_bytes())
    }
}

/// Wraps an XKB keymap into a sealed file or stores as just a string for sending to WlKeyboard over an fd
///
/// This is a reference-counted wrapper over the file.
#[derive(Clone, Debug)]
pub struct KeymapFile(Arc<KeymapFileInner>);

#[derive(Debug)]
struct KeymapFileInner {
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

        Self(Arc::new(KeymapFileInner {
            id: KeymapFileId::for_keymap(&keymap),
            sealed: sealed.ok(),
            keymap,
        }))
    }

    #[cfg(feature = "wayland_frontend")]
    /// Run a closure with the file descriptor to ensure safety
    pub fn with_fd<F>(&self, supports_sealed: bool, cb: F) -> Result<(), std::io::Error>
    where
        F: FnOnce(BorrowedFd<'_>, usize),
    {
        use std::{io::Write, path::PathBuf};

        if let Some(file) = supports_sealed.then_some(self.0.sealed.as_ref()).flatten() {
            cb(file.as_fd(), file.size());
        } else {
            let dir = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);

            let mut file = tempfile::tempfile_in(dir)?;
            file.write_all(self.0.keymap.as_bytes())?;
            file.flush()?;

            cb(file.as_fd(), self.0.keymap.len());
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
    pub fn id(&self) -> KeymapFileId {
        self.0.id
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0.keymap
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

        assert_ne!(keymap_file.id().0, [0_u8; 16]);
    }
}
