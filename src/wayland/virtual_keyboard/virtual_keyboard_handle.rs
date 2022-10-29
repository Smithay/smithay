use std::{
    fmt::Debug,
    os::unix::prelude::AsRawFd,
    sync::{Arc, Mutex},
};

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
pub struct VirtualKeyboardUserData<D: SeatHandler> {
    pub(super) handle: VirtualKeyboardHandle,
    pub(crate) seat: Seat<D>,
}

impl<D: SeatHandler> Debug for VirtualKeyboardUserData<D>
where
    <D as SeatHandler>::KeyboardFocus: Debug,
    <D as SeatHandler>::PointerFocus: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        _: &ZwpVirtualKeyboardV1,
        request: zwp_virtual_keyboard_v1::Request,
        data: &VirtualKeyboardUserData<D>,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_virtual_keyboard_v1::Request::Keymap { format, fd, size } => {
                // This should be wl_keyboard::KeymapFormat::XkbV1,
                // but the protocol does not state the parameter is an enum.
                if format == 1 {
                    let keyboard_handle = data.seat.get_keyboard().unwrap();
                    let internal = keyboard_handle.arc.internal.lock().unwrap();
                    let old_keymap = internal.keymap.get_as_string(xkb::FORMAT_TEXT_V1);
                    let new_keymap = unsafe {
                        xkb::Keymap::new_from_fd(
                            &xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
                            fd.as_raw_fd(),
                            size as usize,
                            format,
                            xkb::KEYMAP_COMPILE_NO_FLAGS,
                        )
                        .unwrap()
                        .unwrap()
                    };
                    if old_keymap != new_keymap.get_as_string(xkb::FORMAT_TEXT_V1) {
                        let mut inner = data.handle.inner.lock().unwrap();
                        inner.old_keymap = Some(old_keymap);
                        keyboard_handle.change_keymap(new_keymap);
                        let known_kbds = &keyboard_handle.arc.known_kbds;
                        for kbd in &*known_kbds.lock().unwrap() {
                            kbd.keymap(KeymapFormat::XkbV1, fd.as_raw_fd(), size);
                        }
                    }
                }
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
        if inner.instances == 0 {
            if let Some(old_keymap) = &inner.old_keymap {
                let keyboard_handle = data.seat.get_keyboard().unwrap();
                let old_keymap = xkb::Keymap::new_from_string(
                    &xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
                    old_keymap.to_string(),
                    xkb::KEYMAP_FORMAT_TEXT_V1,
                    xkb::KEYMAP_COMPILE_NO_FLAGS,
                )
                .unwrap();
                keyboard_handle.change_keymap(old_keymap);
                let keymap_file = &keyboard_handle.arc.keymap.lock().unwrap();
                keymap_file
                    .with_fd(false, |fd, size| {
                        let known_kbds = &keyboard_handle.arc.known_kbds;
                        for kbd in &*known_kbds.lock().unwrap() {
                            kbd.keymap(KeymapFormat::XkbV1, fd.as_raw_fd(), size as u32);
                        }
                    })
                    .ok(); //TODO: log some kind of error here;
            }
        }
    }
}
