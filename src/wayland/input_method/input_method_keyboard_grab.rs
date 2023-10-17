use std::{
    fmt,
    sync::{Arc, Mutex},
};

use wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_keyboard_grab_v2::{
    self, ZwpInputMethodKeyboardGrabV2,
};
use wayland_server::backend::ClientId;
use wayland_server::Dispatch;

use crate::input::{
    keyboard::{
        GrabStartData as KeyboardGrabStartData, KeyboardGrab, KeyboardHandle, KeyboardInnerHandle,
        ModifiersState,
    },
    SeatHandler,
};
use crate::wayland::text_input::TextInputHandle;
use crate::{backend::input::KeyState, utils::Serial};

use super::InputMethodManagerState;

#[derive(Default, Debug)]
pub(crate) struct InputMethodKeyboard {
    pub grab: Option<ZwpInputMethodKeyboardGrabV2>,
    pub text_input_handle: TextInputHandle,
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
        _handle: &mut KeyboardInnerHandle<'_, D>,
        keycode: u32,
        key_state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: u32,
    ) {
        let inner = self.inner.lock().unwrap();
        let keyboard = inner.grab.as_ref().unwrap();
        inner
            .text_input_handle
            .focused_text_input_serial_or_default(serial.0, |serial| {
                keyboard.key(serial, time, keycode, key_state.into());
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
        _state: &mut D,
        _client: ClientId,
        _object: &ZwpInputMethodKeyboardGrabV2,
        data: &InputMethodKeyboardUserData<D>,
    ) {
        data.handle.inner.lock().unwrap().grab = None;
        data.keyboard_handle.unset_grab();
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
