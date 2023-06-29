use std::fs::File;
use std::io::Read;
use std::os::unix::io::{FromRawFd, OwnedFd};
use std::{
    os::unix::io::AsRawFd,
    sync::{Arc, Mutex},
};

use tracing::debug;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_v1::{
    self, ZwpVirtualKeyboardV1,
};
use wayland_server::{
    backend::{ClientId, ObjectId},
    protocol::wl_keyboard::{KeyState, KeymapFormat},
    Client, DataInit, Dispatch, DisplayHandle,
};
use xkbcommon::xkb;

use crate::{
    input::{Seat, SeatHandler},
    utils::SERIAL_COUNTER,
    wayland::seat::{keyboard::for_each_focused_kbds, WaylandFocus},
};

use super::VirtualKeyboardManagerState;

/// Maximum keymap size. Up to 1MiB.
const MAX_KEYMAP_SIZE: usize = 0x100000;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
struct SerializedMods {
    depressed: u32,
    latched: u32,
    locked: u32,
    group: u32,
}

#[derive(Debug, Default)]
pub(crate) struct VirtualKeyboard {
    instances: u8,
    modifiers: Option<SerializedMods>,
    old_keymap: Option<String>,
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
#[derive(Debug)]
pub struct VirtualKeyboardUserData<D: SeatHandler> {
    pub(super) handle: VirtualKeyboardHandle,
    pub(crate) seat: Seat<D>,
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
        _: &ZwpVirtualKeyboardV1,
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
                let keyboard_handle = data.seat.get_keyboard().unwrap();
                let internal = keyboard_handle.arc.internal.lock().unwrap();
                let inner = data.handle.inner.lock().unwrap();
                if let Some(focus) = internal.focus.as_ref().and_then(|f| f.0.wl_surface()) {
                    for_each_focused_kbds(&data.seat, &focus, |kbd| {
                        // This should be wl_keyboard::KeyState,
                        // but the protocol does not state the parameter is an enum.
                        let key_state = if state == 1 {
                            KeyState::Pressed
                        } else {
                            KeyState::Released
                        };
                        kbd.key(SERIAL_COUNTER.next_serial().0, time, key, key_state);
                        if let Some(mods) = inner.modifiers {
                            kbd.modifiers(
                                SERIAL_COUNTER.next_serial().0,
                                mods.depressed,
                                mods.latched,
                                mods.locked,
                                mods.group,
                            );
                        }
                    });
                }
            }
            zwp_virtual_keyboard_v1::Request::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                data.handle.inner.lock().unwrap().modifiers = Some(SerializedMods {
                    depressed: mods_depressed,
                    latched: mods_latched,
                    locked: mods_locked,
                    group,
                });
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
        _virtual_keyboard: ObjectId,
        data: &VirtualKeyboardUserData<D>,
    ) {
        let mut inner = data.handle.inner.lock().unwrap();
        inner.instances -= 1;
        if inner.instances != 0 {
            return;
        }

        let old_keymap = match &inner.old_keymap {
            Some(old_keymap) => old_keymap,
            None => return,
        };

        let keyboard_handle = data.seat.get_keyboard().unwrap();
        let old_keymap = xkb::Keymap::new_from_string(
            &xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            old_keymap.to_string(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        );

        // Restore the old keymap.
        if let Some(old_keymap) = old_keymap {
            keyboard_handle.change_keymap(old_keymap);
        }
    }
}

/// Handle the zwp_virtual_keyboard_v1::keymap request.
fn update_keymap<D>(data: &VirtualKeyboardUserData<D>, format: u32, fd: OwnedFd, size: usize)
where
    D: SeatHandler + 'static,
{
    // Only libxkbcommon compatible keymaps are supported.
    if format != KeymapFormat::XkbV1 as u32 {
        debug!("Unsupported keymap format: {format:?}");
        return;
    }

    // Ignore potentially malicious requests.
    if size > MAX_KEYMAP_SIZE {
        debug!("Excessive keymap size: {size:?}");
        return;
    }

    // Read entire keymap.
    let mut keymap_buffer = vec![0; size];
    let mut file = unsafe { File::from_raw_fd(fd.as_raw_fd()) };
    if let Err(err) = file.read_exact(&mut keymap_buffer) {
        debug!("Could not read keymap: {err}");
        return;
    }
    let mut new_keymap = match String::from_utf8(keymap_buffer) {
        Ok(keymap) => keymap,
        Err(err) => {
            debug!("Invalid utf8 keymap: {err}");
            return;
        }
    };

    // Ignore everything after the first nul byte.
    if let Some(nul_index) = new_keymap.find('\0') {
        new_keymap.truncate(nul_index);
    }

    // Attempt to parse the new keymap.
    let new_keymap = xkb::Keymap::new_from_string(
        &xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
        new_keymap,
        xkb::KEYMAP_FORMAT_TEXT_V1,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    );
    let new_keymap = match new_keymap {
        Some(keymap) => keymap,
        None => {
            debug!("Invalid libxkbcommon keymap");
            return;
        }
    };

    // Get old keymap to allow restoring to it later.
    let keyboard_handle = data.seat.get_keyboard().unwrap();
    let internal = keyboard_handle.arc.internal.lock().unwrap();
    let old_keymap = internal.keymap.get_as_string(xkb::FORMAT_TEXT_V1);

    if old_keymap != new_keymap.get_as_string(xkb::FORMAT_TEXT_V1) {
        let mut inner = data.handle.inner.lock().unwrap();
        inner.old_keymap = Some(old_keymap);
        keyboard_handle.change_keymap(new_keymap);
    }
}
