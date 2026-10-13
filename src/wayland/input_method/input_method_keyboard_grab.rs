use std::{
    fmt,
    sync::{Arc, Mutex},
};

use tracing::warn;
use wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_keyboard_grab_v2::{
    self, ZwpInputMethodKeyboardGrabV2,
};
use wayland_server::{backend::ClientId, protocol::wl_keyboard::KeymapFormat};

use crate::input::{
    SeatHandler,
    keyboard::{
        GrabStartData as KeyboardGrabStartData, KeyboardGrab, KeyboardHandle, KeyboardInnerHandle,
        KeymapFileId, ModifiersState, SerializedMods,
    },
};
use crate::wayland::text_input::TextInputHandle;
use crate::{
    backend::input::{InputTime, KeyState, Keycode},
    utils::Serial,
    wayland::Dispatch2,
};

#[derive(Default, Debug)]
pub(crate) struct InputMethodKeyboard {
    pub grab: Option<ZwpInputMethodKeyboardGrabV2>,
    pub text_input_handle: TextInputHandle,
    pub keymap_id: Option<KeymapFileId>,
}

/// Handle to an input method instance
#[derive(Default, Debug, Clone)]
pub struct InputMethodKeyboardGrab {
    pub(crate) inner: Arc<Mutex<InputMethodKeyboard>>,
}

impl<D> KeyboardGrab<D> for InputMethodKeyboardGrab
where
    D: SeatHandler + 'static,
{
    fn input(
        &mut self,
        _data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        keycode: Keycode,
        key_state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: InputTime,
    ) {
        let mut inner = self.inner.lock().unwrap();

        // The keyboard's keymap can change while the grab is active (e.g. when a virtual keyboard
        // with its own keymap is in use, e.g., wtype's synthetic keymap), and the keycodes must be
        // interpreted using it, so keep the grab in sync like wlroots does. The id is a hash of the
        // content, so an input method sending the same keymap back through its own virtual keyboard
        // will not result in a loop of keymap updates.
        let keymap_changed = handle.with_keymap_file(|keymap_file| {
            let id = keymap_file.id();
            if inner.keymap_id == Some(id) {
                return false;
            }
            let keyboard = inner.grab.as_ref().unwrap();
            let res = keymap_file.with_fd(false, |fd, size| {
                keyboard.keymap(KeymapFormat::XkbV1, fd, size as u32);
            });
            if let Err(err) = res {
                warn!(err = ?err, "Failed to send keymap to client");
                return false;
            }
            inner.keymap_id = Some(id);
            true
        });

        // The client resets its state when it receives a new keymap, so send the full modifier
        // state before the key in that case. Otherwise, only send the modifiers if they changed.
        let (mods_before_key, mods_after_key) = if keymap_changed {
            (Some(handle.modifier_state().serialized), None)
        } else {
            (None, modifiers.map(|m| m.serialized))
        };

        let keyboard = inner.grab.as_ref().unwrap();
        let send_modifiers = |serial, serialized: SerializedMods| {
            keyboard.modifiers(
                serial,
                serialized.depressed,
                serialized.latched,
                serialized.locked,
                serialized.layout_effective,
            )
        };
        inner
            .text_input_handle
            .active_text_input_serial_or_default(serial.0, |serial| {
                if let Some(serialized) = mods_before_key {
                    send_modifiers(serial, serialized);
                }
                keyboard.key(serial, time.millis(), keycode.raw() - 8, key_state.into());
                if let Some(serialized) = mods_after_key {
                    send_modifiers(serial, serialized);
                }
            });
    }

    fn set_focus(
        &mut self,
        data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        focus: Option<<D as SeatHandler>::KeyboardFocus>,
        serial: crate::utils::Serial,
    ) {
        handle.set_focus(data, focus, serial)
    }

    fn start_data(&self) -> &KeyboardGrabStartData<D> {
        &KeyboardGrabStartData { focus: None }
    }

    fn unset(&mut self, _data: &mut D) {}
}

/// User data of ZwpInputKeyboardGrabV2 object
pub struct InputMethodKeyboardUserData<D: SeatHandler> {
    pub(super) handle: InputMethodKeyboardGrab,
    pub(crate) keyboard_handle: KeyboardHandle<D>,
}

impl<D: SeatHandler> fmt::Debug for InputMethodKeyboardUserData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputMethodKeyboardUserData")
            .field("handle", &self.handle)
            .field("keyboard_handle", &self.keyboard_handle)
            .finish()
    }
}

impl<D: SeatHandler + 'static> Dispatch2<ZwpInputMethodKeyboardGrabV2, D> for InputMethodKeyboardUserData<D> {
    fn destroyed(&self, state: &mut D, _client: ClientId, _object: &ZwpInputMethodKeyboardGrabV2) {
        let mut inner = self.handle.inner.lock().unwrap();
        inner.grab = None;
        inner.keymap_id = None;
        drop(inner);
        self.keyboard_handle.unset_grab(state);
    }

    fn request(
        &self,
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &ZwpInputMethodKeyboardGrabV2,
        request: zwp_input_method_keyboard_grab_v2::Request,
        _dhandle: &wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_input_method_keyboard_grab_v2::Request::Release => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }
}
