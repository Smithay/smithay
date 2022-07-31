use std::sync::{Arc, Mutex};

use wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_keyboard_grab_v2::{
    self, ZwpInputMethodKeyboardGrabV2,
};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::Dispatch;

use crate::wayland::seat::keyboard::KeymapFile;
use crate::wayland::seat::KeyboardGrabStartData;
use crate::wayland::seat::{KeyboardGrab, KeyboardHandle, KeyboardInnerHandle};
use crate::wayland::text_input_manager::TextInputHandle;

use super::InputMethodManagerState;

#[derive(Default, Debug)]
pub(crate) struct InputMethodKeyboard {
    pub grab: Option<ZwpInputMethodKeyboardGrabV2>,
    pub repeat_delay: i32,
    pub repeat_rate: i32,
    pub keymap_file: Option<KeymapFile>,
    pub text_input_handle: Option<TextInputHandle>,
}

/// Handle to an input method instance
#[derive(Default, Debug, Clone)]
pub struct InputMethodKeyboardGrab {
    pub(crate) inner: Arc<Mutex<InputMethodKeyboard>>,
}

impl KeyboardGrab for InputMethodKeyboardGrab {
    fn input(
        &mut self,
        _dh: &wayland_server::DisplayHandle,
        _handle: &mut crate::wayland::seat::KeyboardInnerHandle<'_>,
        keycode: u32,
        key_state: wayland_server::protocol::wl_keyboard::KeyState,
        modifiers: Option<(u32, u32, u32, u32)>,
        _serial: crate::wayland::Serial,
        time: u32,
    ) {
        let inner = self.inner.lock().unwrap();
        let keyboard = inner.grab.as_ref().unwrap();
        inner
            .text_input_handle
            .as_ref()
            .unwrap()
            .with_focused_text_input(|_, _, serial| {
                if let Some((dep, la, lo, gr)) = modifiers {
                    keyboard.modifiers(*serial, dep, la, lo, gr)
                }
                keyboard.key(*serial, time, keycode, key_state);
            });
    }

    fn set_focus(
        &mut self,
        _dh: &wayland_server::DisplayHandle,
        handle: &mut KeyboardInnerHandle<'_>,
        focus: Option<&WlSurface>,
        serial: crate::wayland::Serial,
    ) {
        let inner = self.inner.lock().unwrap();
        inner.text_input_handle.as_ref().unwrap().set_focus(focus);
        handle.set_focus(focus, serial)
    }

    fn start_data(&self) -> &KeyboardGrabStartData {
        &KeyboardGrabStartData { focus: None }
    }
}

/// User data of ZwpInputKeyboardGrabV2 object
#[derive(Debug)]
pub struct InputMethodKeyboardUserData {
    pub(super) handle: InputMethodKeyboardGrab,
    pub(crate) keyboard_handle: KeyboardHandle,
}

impl<D> Dispatch<ZwpInputMethodKeyboardGrabV2, InputMethodKeyboardUserData, D> for InputMethodManagerState {
    fn destroyed(_state: &mut D, _client: ClientId, _id: ObjectId, data: &InputMethodKeyboardUserData) {
        data.handle.inner.lock().unwrap().grab = None;
        data.keyboard_handle.unset_grab();
    }

    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &ZwpInputMethodKeyboardGrabV2,
        request: zwp_input_method_keyboard_grab_v2::Request,
        _data: &InputMethodKeyboardUserData,
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
