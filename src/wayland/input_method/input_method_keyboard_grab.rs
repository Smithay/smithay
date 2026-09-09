use std::{
    fmt,
    sync::{Arc, Mutex},
};

use wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_keyboard_grab_v2::{
    self, ZwpInputMethodKeyboardGrabV2,
};
use wayland_server::backend::ClientId;

use crate::input::{
    Seat, SeatHandler,
    keyboard::{
        KeyboardHandle, KeyboardInputInterception, KeyboardInputInterceptor, KeyboardSource, ModifiersState,
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
}

/// Handle to an input method instance
#[derive(Default, Debug, Clone)]
pub struct InputMethodKeyboardGrab {
    pub(crate) inner: Arc<Mutex<InputMethodKeyboard>>,
}

impl<D> KeyboardInputInterceptor<D> for InputMethodKeyboardGrab
where
    D: SeatHandler + 'static,
{
    fn input(
        &mut self,
        _data: &mut D,
        _seat: &Seat<D>,
        source: KeyboardSource,
        keycode: Keycode,
        key_state: KeyState,
        modifiers: Option<ModifiersState>,
        serial: Serial,
        time: InputTime,
    ) -> KeyboardInputInterception {
        // The protocol grab only applies to physical keyboard input.
        if source != KeyboardSource::Physical {
            return KeyboardInputInterception::Forward;
        }

        let (keyboard, text_input_handle) = {
            let inner = self.inner.lock().unwrap();
            let Some(keyboard) = inner.grab.clone() else {
                // The grab may have been destroyed before the interceptor is cleared.
                return KeyboardInputInterception::Forward;
            };
            (keyboard, inner.text_input_handle.clone())
        };

        text_input_handle.active_text_input_serial_or_default(serial.0, |serial| {
            keyboard.key(serial, time.millis(), keycode.raw() - 8, key_state.into());
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

        KeyboardInputInterception::Intercept
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

impl<D: SeatHandler + 'static> Dispatch2<ZwpInputMethodKeyboardGrabV2, D> for InputMethodKeyboardUserData<D> {
    fn destroyed(&self, _state: &mut D, _client: ClientId, object: &ZwpInputMethodKeyboardGrabV2) {
        let was_current = {
            let mut inner = self.handle.inner.lock().unwrap();
            if inner.grab.as_ref().is_some_and(|grab| grab == object) {
                inner.grab = None;
                true
            } else {
                false
            }
        };

        if was_current {
            self.keyboard_handle.unset_input_interceptor();
        }
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
