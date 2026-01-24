use std::{
    fmt,
    fs::File,
    os::unix::io::OwnedFd,
    sync::{Arc, Mutex},
};

use memmap2::MmapOptions;
use tracing::debug;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_v1::Error::NoKeymap;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_v1::{
    self, ZwpVirtualKeyboardV1,
};
use wayland_server::protocol::wl_keyboard::KeymapFormat;
use wayland_server::{backend::ClientId, Client, DataInit, Dispatch, DisplayHandle, Resource};
use xkbcommon::xkb;

use crate::input::keyboard::{Keymap, ModifiersState};
use crate::wayland::input_method::InputMethodSeat;
use crate::{
    input::{Seat, SeatHandler},
    utils::SERIAL_COUNTER,
    wayland::seat::{keyboard::for_each_focused_kbds, WaylandFocus},
};

use crate::backend::input::KeyState;

use super::{VirtualKeyboardHandler, VirtualKeyboardManagerState};

#[derive(Debug, Default)]
pub(crate) struct VirtualKeyboard {
    state: Option<VirtualKeyboardState>,
}

struct VirtualKeyboardState {
    keymap: Keymap,
    mods: ModifiersState,
    state: xkb::State,
    pressed_keys: Vec<u32>,
    pressed_keys_internal: Vec<u32>,
}

impl fmt::Debug for VirtualKeyboardState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualKeyboardState")
            .field("keymap", &self.keymap)
            .field("mods", &self.mods)
            .field("state", &self.state.get_raw_ptr())
            .finish()
    }
}

// This is OK because all parts of `xkb` will remain on the
// same thread
unsafe impl Send for VirtualKeyboard {}

/// Handle to a virtual keyboard instance
#[derive(Debug, Clone, Default)]
pub(crate) struct VirtualKeyboardHandle {
    pub(crate) inner: Arc<Mutex<VirtualKeyboard>>,
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
    D: VirtualKeyboardHandler,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
{
    fn request(
        user_data: &mut D,
        client: &Client,
        virtual_keyboard: &ZwpVirtualKeyboardV1,
        request: zwp_virtual_keyboard_v1::Request,
        data: &VirtualKeyboardUserData<D>,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        let ime_keyboard_grabbed = {
            let input_method = data.seat.input_method().inner.lock().unwrap();
            let keyboard_grab = input_method.keyboard_grab.inner.lock().unwrap();
            keyboard_grab.grab.clone()
        };
        match request {
            zwp_virtual_keyboard_v1::Request::Keymap { format, fd, size } => {
                update_keymap(user_data, data, format, fd, size as usize);
            }

            zwp_virtual_keyboard_v1::Request::Key { time, key, state } => {
                // Ensure keymap was initialized.
                let mut virtual_data = data.handle.inner.lock().unwrap();
                let vk_state = match virtual_data.state.as_mut() {
                    Some(vk_state) => vk_state,
                    None => {
                        virtual_keyboard.post_error(NoKeymap, "`key` sent before keymap.");
                        return;
                    }
                };
                // Ensure virtual keyboard's keymap is active.
                let keyboard_handle = data.seat.get_keyboard().unwrap();
                if ime_keyboard_grabbed
                    .map(|grab| grab.client().as_ref() == Some(client))
                    .unwrap_or(false)
                {
                    let old_modifiers = keyboard_handle.modifier_state();
                    let old_keymap = keyboard_handle.get_keymap();
                    let _ = keyboard_handle.set_keymap(user_data, &vk_state.keymap);
                    use wayland_server::protocol::wl_keyboard::KeyState;
                    let mut internal = keyboard_handle.arc.internal.lock().unwrap();
                    let focus = internal.focus.as_mut().map(|(focus, _)| focus);
                    if let Some(wl_surface) = focus.and_then(|f| f.wl_surface()) {
                        for_each_focused_kbds(&data.seat, &wl_surface, |kbd| {
                            // This should be wl_keyboard::KeyState, but the protocol does not state
                            // the parameter is an enum.
                            let key_state = if state == 1 {
                                vk_state.pressed_keys_internal.push(key);
                                KeyState::Pressed
                            } else {
                                vk_state.pressed_keys_internal.retain(|&x| x == key);
                                KeyState::Released
                            };

                            kbd.key(SERIAL_COUNTER.next_serial().0, time, key, key_state);
                        });
                    }
                    drop(internal);
                    let _ = keyboard_handle.set_keymap(user_data, &old_keymap);
                    let _ = keyboard_handle.set_modifier_state(old_modifiers);
                    keyboard_handle.advertise_modifier_state(user_data);
                } else {
                    let old_modifiers = keyboard_handle.modifier_state();
                    let old_keymap = keyboard_handle.get_keymap();
                    let _ = keyboard_handle.set_keymap(user_data, &vk_state.keymap);
                    let key_state = if state == 1 {
                        vk_state.pressed_keys.push(key);
                        KeyState::Pressed
                    } else {
                        vk_state.pressed_keys.retain(|&x| x == key);
                        KeyState::Released
                    };
                    user_data.on_keyboard_event((key + 8).into(), key_state, time, keyboard_handle.clone());
                    let _ = keyboard_handle.set_keymap(user_data, &old_keymap);
                    let _ = keyboard_handle.set_modifier_state(old_modifiers);
                    keyboard_handle.advertise_modifier_state(user_data);
                }
            }
            zwp_virtual_keyboard_v1::Request::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                // Ensure keymap was initialized.
                let mut virtual_data = data.handle.inner.lock().unwrap();
                let state = match virtual_data.state.as_mut() {
                    Some(state) => state,
                    None => {
                        virtual_keyboard.post_error(NoKeymap, "`modifiers` sent before keymap.");
                        return;
                    }
                };

                // Update virtual keyboard's modifier state.
                state
                    .state
                    .update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                state.mods.update_with(&state.state);

                // Ensure virtual keyboard's keymap is active.
                let keyboard_handle = data.seat.get_keyboard().unwrap();
                let old_modifiers = keyboard_handle.modifier_state();
                let old_keymap = keyboard_handle.get_keymap();
                let _ = keyboard_handle.set_keymap(user_data, &state.keymap);
                let _ = keyboard_handle.set_modifier_state(state.mods);
                keyboard_handle.advertise_modifier_state(user_data);
                if ime_keyboard_grabbed
                    .map(|grab| grab.client().as_ref() == Some(client))
                    .unwrap_or(false)
                {
                    user_data.on_keyboard_modifiers(
                        mods_depressed,
                        mods_latched,
                        mods_locked,
                        keyboard_handle.clone(),
                    );
                }
                let _ = keyboard_handle.set_keymap(user_data, &old_keymap);
                let _ = keyboard_handle.set_modifier_state(old_modifiers);
                keyboard_handle.advertise_modifier_state(user_data);
            }
            zwp_virtual_keyboard_v1::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        _virtual_keyboard: &ZwpVirtualKeyboardV1,
        data: &VirtualKeyboardUserData<D>,
    ) {
        release_key(state, data);
    }
}

fn release_key<D>(user_data: &mut D, data: &VirtualKeyboardUserData<D>)
where
    D: SeatHandler + 'static + VirtualKeyboardHandler,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
{
    let mut virtual_data = data.handle.inner.lock().unwrap();
    let vk_state = match virtual_data.state.as_mut() {
        Some(vk_state) => vk_state,
        None => {
            return;
        }
    };
    let pressed_keys = &mut vk_state.pressed_keys;
    let keyboard_handle = data.seat.get_keyboard().unwrap();
    let old_keymap = keyboard_handle.get_keymap();
    let _ = keyboard_handle.set_keymap(user_data, &vk_state.keymap);
    for i in pressed_keys.drain(..) {
        user_data.on_keyboard_event((i + 8).into(), KeyState::Released, 0, keyboard_handle.clone());
    }
    let pressed_keys_internal = &mut vk_state.pressed_keys_internal;
    let mut internal = keyboard_handle.arc.internal.lock().unwrap();
    let focus = internal.focus.as_mut().map(|(focus, _)| focus);
    for i in pressed_keys_internal.drain(..) {
        if let Some(wl_surface) = focus.as_ref().and_then(|f| f.wl_surface()) {
            for_each_focused_kbds(&data.seat, &wl_surface, |kbd| {
                kbd.key(SERIAL_COUNTER.next_serial().0, 0, i, KeyState::Released.into());
            });
        }
    }
    drop(internal);
    let _ = keyboard_handle.set_keymap(user_data, &old_keymap);
    keyboard_handle.advertise_modifier_state(user_data);
}

/// Handle the zwp_virtual_keyboard_v1::keymap request.
///
/// The `true` returns when keymap was properly loaded.
fn update_keymap<D>(
    user_data: &mut D,
    data: &VirtualKeyboardUserData<D>,
    format: u32,
    fd: OwnedFd,
    size: usize,
) where
    D: SeatHandler + 'static + VirtualKeyboardHandler,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
{
    release_key(user_data, data);
    // Only libxkbcommon compatible keymaps are supported.
    if format != KeymapFormat::XkbV1 as u32 {
        debug!("Unsupported keymap format: {format:?}");
        return;
    }

    let map = unsafe {
        MmapOptions::new()
            .len(size)
            .map_copy_read_only(&File::from(fd))
            .unwrap()
    };
    let keymap_string = String::from_utf8_lossy(&map[..]).to_string();
    // Store active virtual keyboard map.
    let mut inner = data.handle.inner.lock().unwrap();
    let mods = inner.state.take().map(|state| state.mods).unwrap_or_default();
    let keyboard_handle = data.seat.get_keyboard().unwrap();
    let old_keymap = keyboard_handle.get_keymap().clone();
    let keymap = {
        let _ = keyboard_handle.set_keymap_from_string(user_data, keymap_string);
        keyboard_handle.get_keymap().clone()
    };
    let state = xkb::State::new(keymap.keymap());
    inner.state = Some(VirtualKeyboardState {
        mods,
        keymap: keymap.clone(),
        state,
        pressed_keys: Vec::<u32>::new(),
        pressed_keys_internal: Vec::<u32>::new(),
    });
    let _ = keyboard_handle.set_keymap(user_data, &old_keymap);
}
