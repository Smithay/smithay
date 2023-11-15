use std::os::unix::io::OwnedFd;
use std::{
    fmt,
    sync::{Arc, Mutex},
};

use tracing::debug;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_v1::Error::NoKeymap;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_v1::{
    self, ZwpVirtualKeyboardV1,
};
use wayland_server::{
    backend::ClientId,
    protocol::wl_keyboard::{KeyState, KeymapFormat},
    Client, DataInit, Dispatch, DisplayHandle, Resource,
};
use xkbcommon::xkb;

use crate::input::keyboard::KeymapFile;
use crate::{
    input::{Seat, SeatHandler},
    utils::SERIAL_COUNTER,
    wayland::seat::{keyboard::for_each_focused_kbds, WaylandFocus},
};

use super::VirtualKeyboardManagerState;

#[derive(Debug, Default)]
pub(crate) struct VirtualKeyboard {
    instances: u8,
    keymap: Option<KeymapFile>,
}

/// Handle to a virtual keyboard instance
#[derive(Debug, Clone, Default)]
pub(crate) struct VirtualKeyboardHandle {
    pub(crate) inner: Arc<Mutex<VirtualKeyboard>>,
}

impl VirtualKeyboardHandle {
    pub(super) fn count_instance(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.instances += 1;
    }
}

/// User data of ZwpVirtualKeyboardV1 object
pub struct VirtualKeyboardUserData<D: SeatHandler> {
    pub(super) handle: VirtualKeyboardHandle,
    pub(crate) seat: Seat<D>,
}

impl<D: SeatHandler> fmt::Debug for VirtualKeyboardUserData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualKeyboardUserData")
            .field("handle", &self.handle)
            .field("seat", &self.seat.arc)
            .finish()
    }
}

impl<D> Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData<D>, D> for VirtualKeyboardManagerState
where
    D: Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData<D>>,
    D: SeatHandler + 'static,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
{
    fn request(
        _data: &mut D,
        _client: &Client,
        virtual_keyboard: &ZwpVirtualKeyboardV1,
        request: zwp_virtual_keyboard_v1::Request,
        data: &VirtualKeyboardUserData<D>,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_virtual_keyboard_v1::Request::Keymap { format, fd, size } => {
                update_keymap(data, format, fd, size as usize);
            }
            zwp_virtual_keyboard_v1::Request::Key { time, key, state } => {
                // Ensure keymap was initialized.
                let virtual_data = data.handle.inner.lock().unwrap();
                let keymap = match &virtual_data.keymap {
                    Some(keymap) => keymap,
                    None => {
                        virtual_keyboard.post_error(NoKeymap, "`key` sent before keymap.");
                        return;
                    }
                };

                // Ensure virtual keyboard's keymap is active.
                let keyboard_handle = data.seat.get_keyboard().unwrap();
                keyboard_handle.send_keymap(keymap);

                let internal = keyboard_handle.arc.internal.lock().unwrap();
                if let Some(focus) = internal.focus.as_ref().and_then(|f| f.0.wl_surface()) {
                    for_each_focused_kbds(&data.seat, &focus, |kbd| {
                        // This should be wl_keyboard::KeyState, but the protocol does not state
                        // the parameter is an enum.
                        let key_state = if state == 1 {
                            KeyState::Pressed
                        } else {
                            KeyState::Released
                        };

                        kbd.key(SERIAL_COUNTER.next_serial().0, time, key, key_state);
                    });
                }
            }
            zwp_virtual_keyboard_v1::Request::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                // Ensure keymap was initialized.
                let virtual_data = data.handle.inner.lock().unwrap();
                let keymap = match &virtual_data.keymap {
                    Some(keymap) => keymap,
                    None => {
                        virtual_keyboard.post_error(NoKeymap, "`modifiers` sent before keymap.");
                        return;
                    }
                };

                // Ensure virtual keyboard's keymap is active.
                let keyboard_handle = data.seat.get_keyboard().unwrap();
                keyboard_handle.send_keymap(keymap);

                let internal = keyboard_handle.arc.internal.lock().unwrap();
                if let Some(focus) = internal.focus.as_ref().and_then(|f| f.0.wl_surface()) {
                    for_each_focused_kbds(&data.seat, &focus, |kbd| {
                        kbd.modifiers(
                            SERIAL_COUNTER.next_serial().0,
                            mods_depressed,
                            mods_latched,
                            mods_locked,
                            group,
                        );
                    });
                }
            }
            zwp_virtual_keyboard_v1::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _client: ClientId,
        _virtual_keyboard: &ZwpVirtualKeyboardV1,
        _data: &VirtualKeyboardUserData<D>,
    ) {
    }
}

/// Handle the zwp_virtual_keyboard_v1::keymap request.
///
/// The `true` returns when keymap was properly loaded.
fn update_keymap<D>(data: &VirtualKeyboardUserData<D>, format: u32, fd: OwnedFd, size: usize)
where
    D: SeatHandler + 'static,
{
    // Only libxkbcommon compatible keymaps are supported.
    if format != KeymapFormat::XkbV1 as u32 {
        debug!("Unsupported keymap format: {format:?}");
        return;
    }

    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    // SAFETY: we can map the keymap into the memory.
    let new_keymap = match unsafe {
        xkb::Keymap::new_from_fd(
            &context,
            fd,
            size,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
    } {
        Ok(Some(new_keymap)) => new_keymap,
        Ok(None) => {
            debug!("Invalid libxkbcommon keymap");
            return;
        }
        Err(err) => {
            debug!("Could not map the keymap: {err:?}");
            return;
        }
    };

    // Store active virtual keyboard map.
    let mut inner = data.handle.inner.lock().unwrap();
    inner.keymap = Some(KeymapFile::new(&new_keymap));
}
