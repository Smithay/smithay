use std::{
    fmt,
    sync::{Arc, Mutex},
};

use tracing::info;
use wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_keyboard_grab_v2::{
    self, ZwpInputMethodKeyboardGrabV2,
};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::Dispatch;

use crate::input::{
    keyboard::{
        GrabStartData as KeyboardGrabStartData, KeyboardGrab, KeyboardHandle, KeyboardInnerHandle,
        ModifiersState,
    },
    SeatHandler,
};
use crate::wayland::text_input::TextInputHandle;
use crate::{
    backend::input::{KeyState, Keycode},
    utils::Serial,
};

use super::InputMethodManagerState;

#[derive(Default, Debug)]
pub(crate) struct InputMethodKeyboard {
    pub grab: Option<ZwpInputMethodKeyboardGrabV2>,
    pub text_input_handle: TextInputHandle,
    pub active_input_method_id: Option<ObjectId>,
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
        time: u32,
    ) {
        let inner = self.inner.lock().unwrap();

        // Check if there's an active input method
        let is_active = inner.active_input_method_id.is_some();

        // Check if there's an active text input with focus
        let has_text_input = inner.text_input_handle.has_active_text_input();

        info!(
            "InputMethodKeyboardGrab: Checking active state - active_input_method_id: {:?}, is_active: {}, has_text_input: {}",
            inner.active_input_method_id, is_active, has_text_input
        );

        // Only forward to IME if both an IME is active AND a text input has focus
        if !is_active || !has_text_input {
            // No active input method or no text input focus, forward to normal keyboard handling
            info!(
                "InputMethodKeyboardGrab: No active IME or no text input focus, forwarding to keyboard - keycode: {}, state: {:?}",
                keycode.raw(),
                key_state
            );
            drop(inner);
            handle.input(_data, keycode, key_state, modifiers, serial, time);
            return;
        }

        // Active input method with text input focus - forward to it
        info!(
            "InputMethodKeyboardGrab: Received keyboard input - keycode: {}, state: {:?}, serial: {:?}, active_id: {:?}",
            keycode.raw(),
            key_state,
            serial,
            inner.active_input_method_id
        );

        let keyboard = inner.grab.as_ref().unwrap();
        info!(
            "InputMethodKeyboardGrab: Forwarding to grab object, active_input_method_id: {:?}",
            inner.active_input_method_id
        );

        inner
            .text_input_handle
            .active_text_input_serial_or_default(serial.0, |serial| {
                info!(
                    "InputMethodKeyboardGrab: Sending key event to IME - keycode: {}, state: {:?}",
                    keycode.raw() - 8,
                    key_state
                );
                keyboard.key(serial, time, keycode.raw() - 8, key_state.into());
                if let Some(serialized) = modifiers.map(|m| m.serialized) {
                    keyboard.modifiers(
                        serial,
                        serialized.depressed,
                        serialized.latched,
                        serialized.locked,
                        serialized.layout_effective,
                    )
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

impl<D: SeatHandler + 'static> Dispatch<ZwpInputMethodKeyboardGrabV2, InputMethodKeyboardUserData<D>, D>
    for InputMethodManagerState
{
    fn destroyed(
        state: &mut D,
        _client: ClientId,
        _object: &ZwpInputMethodKeyboardGrabV2,
        data: &InputMethodKeyboardUserData<D>,
    ) {
        info!("InputMethodKeyboardGrab: Keyboard grab destroyed, unsetting grab");
        data.handle.inner.lock().unwrap().grab = None;
        data.keyboard_handle.unset_grab(state);
    }

    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &ZwpInputMethodKeyboardGrabV2,
        request: zwp_input_method_keyboard_grab_v2::Request,
        _data: &InputMethodKeyboardUserData<D>,
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
