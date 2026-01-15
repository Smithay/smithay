use std::fmt;

use wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_keyboard_grab_v2::{
    self, ZwpInputMethodKeyboardGrabV2,
};
use wayland_server::backend::ClientId;
use wayland_server::{Dispatch, Resource};

use crate::{
    backend::input::{KeyState, Keycode},
    input::{
        keyboard::{
            GrabStartData as KeyboardGrabStartData, KeyboardGrab, KeyboardHandle, KeyboardInnerHandle,
            ModifiersState,
        },
        SeatHandler,
    },
    utils::Serial,
};

use super::{InputMethodHandle, InputMethodManagerState};

impl<D> KeyboardGrab<D> for InputMethodHandle
where
    D: SeatHandler + 'static,
{
    fn input(
        &mut self,
        data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        keycode: Keycode,
        key_state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: u32,
    ) {
        // Get the keyboard grab and text input handle from the active instance
        let grab_info = self.with_instance(|inst| {
            (
                inst.keyboard_grab.clone(),
                inst.text_input_handle.clone(),
                inst.text_input_handle.has_active_text_input(),
            )
        });

        let Some((Some(keyboard), text_input_handle, has_text_input)) = grab_info else {
            // No active input method, no keyboard grab, or no text input - forward to normal keyboard handling
            handle.input(data, keycode, key_state, modifiers, serial, time);
            return;
        };

        if !has_text_input {
            // No text input focus, forward to normal keyboard handling
            handle.input(data, keycode, key_state, modifiers, serial, time);
            return;
        }

        // Forward to IME
        text_input_handle.active_text_input_serial_or_default(serial.0, |serial| {
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
        serial: Serial,
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
    pub(crate) handle: InputMethodHandle,
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
        // Clear the grab from the instance
        let mut inner = data.handle.inner.lock().unwrap();
        if let Some(inst) = inner
            .instances
            .iter_mut()
            .find(|i| i.keyboard_grab.as_ref().map(|g| g.id()) == Some(_object.id()))
        {
            inst.keyboard_grab = None;
        }

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
