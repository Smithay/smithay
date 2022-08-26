use std::{
    ffi::CString,
    fmt::Debug,
    sync::{Arc, Mutex},
};

use wayland_backend::server::{ClientId, ObjectId};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_v1::{
    self, ZwpVirtualKeyboardV1,
};
use wayland_server::{
    protocol::wl_keyboard::{KeyState, KeymapFormat},
    Client, DataInit, Dispatch, DisplayHandle, Resource,
};
use xkbcommon::xkb;

use crate::{
    input::{keyboard::KeymapFile, Seat, SeatHandler},
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
    pub instances: Vec<ZwpVirtualKeyboardV1>,
    modifiers: Option<SerializedMods>,
    old_keymap: Option<KeymapFile>,
}

/// Handle to a virtual keyboard instance
#[derive(Debug, Clone, Default)]
pub(crate) struct VirtualKeyboardHandle {
    pub(crate) inner: Arc<Mutex<VirtualKeyboard>>,
}

impl VirtualKeyboardHandle {
    pub(super) fn add_instance<D>(&self, instance: &ZwpVirtualKeyboardV1) {
        let mut inner = self.inner.lock().unwrap();
        inner.instances.push(instance.clone());
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
                if format == 1 {
                    let keyboard_handle = data.seat.get_keyboard().unwrap();
                    let mut internal = keyboard_handle.arc.internal.lock().unwrap();
                    let old_keymap = internal.keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
                    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                    let new_keymap = xkb::Keymap::new_from_fd(
                        &context,
                        fd,
                        size as usize,
                        format,
                        xkb::KEYMAP_COMPILE_NO_FLAGS,
                    )
                    .unwrap();
                    if old_keymap != new_keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1) {
                        let mut inner = data.handle.inner.lock().unwrap();
                        let log = crate::slog_or_fallback(None);
                        if inner.old_keymap.is_none() {
                            let old_keymap = CString::new(old_keymap)
                                .expect("Keymap should not contain interior null bytes");
                            inner.old_keymap = Some(KeymapFile::new(old_keymap, log));
                        }
                        internal.keymap = new_keymap;
                        let known_kbds = &keyboard_handle.arc.known_kbds;
                        for kbd in &*known_kbds.lock().unwrap() {
                            kbd.keymap(KeymapFormat::XkbV1, fd, size);
                        }
                    }
                }
            }
            zwp_virtual_keyboard_v1::Request::Key { time, key, state } => {
                let keyboard_handle = data.seat.get_keyboard().unwrap();
                let internal = keyboard_handle.arc.internal.lock().unwrap();
                let inner = data.handle.inner.lock().unwrap();
                if let Some(focus) = internal.focus.as_ref().and_then(|f| f.0.wl_surface()) {
                    for_each_focused_kbds(&data.seat, focus, |kbd| {
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
        virtual_keyboard: ObjectId,
        data: &VirtualKeyboardUserData<D>,
    ) {
        let mut inner = data.handle.inner.lock().unwrap();
        inner.instances.retain(|i| i.id() != virtual_keyboard);
        if inner.instances.is_empty() {
            if let Some(old_keymap) = &inner.old_keymap {
                old_keymap
                    .with_fd(false, |fd, size| {
                        let keyboard_handle = data.seat.get_keyboard().unwrap();
                        let known_kbds = &keyboard_handle.arc.known_kbds;
                        for kbd in &*known_kbds.lock().unwrap() {
                            kbd.keymap(KeymapFormat::XkbV1, fd, size as _);
                        }
                        let mut internal = keyboard_handle.arc.internal.lock().unwrap();
                        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                        internal.keymap = xkb::Keymap::new_from_fd(
                            &context,
                            fd,
                            size,
                            KeymapFormat::XkbV1.into(),
                            xkb::KEYMAP_COMPILE_NO_FLAGS,
                        )
                        .unwrap();
                    })
                    .unwrap(); //TODO: log some kind of error here;
                inner.old_keymap = None;
            }
        }
    }
}
