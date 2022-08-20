use std::sync::{Arc, Mutex};

use wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_keyboard_grab_v2::{
    self, ZwpInputMethodKeyboardGrabV2,
};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::Dispatch;

use crate::backend::input::KeyState;
use crate::input::{
    keyboard::{
        GrabStartData as KeyboardGrabStartData, KeyboardGrab, KeyboardHandle, KeyboardInnerHandle,
        KeymapFile, ModifiersState,
    },
    SeatHandler,
};
use crate::wayland::{seat::WaylandFocus, text_input::TextInputHandle};

use super::InputMethodManagerState;
use super::input_method_popup_surface::InputMethodPopupSurfaceHandle;

#[derive(Default, Debug)]
pub(crate) struct InputMethodKeyboard {
    pub grab: Option<ZwpInputMethodKeyboardGrabV2>,
    pub repeat_delay: i32,
    pub repeat_rate: i32,
    pub keymap_file: Option<KeymapFile>,
    pub text_input_handle: Option<TextInputHandle>,
    pub popup: InputMethodPopupSurfaceHandle
}

/// Handle to an input method instance
#[derive(Default, Debug, Clone)]
pub struct InputMethodKeyboardGrab {
    pub(crate) inner: Arc<Mutex<InputMethodKeyboard>>,
}

impl<D> KeyboardGrab<D> for InputMethodKeyboardGrab
where
    D: SeatHandler + 'static,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
{
    fn input(
        &mut self,
        _data: &mut D,
        _handle: &mut KeyboardInnerHandle<'_, D>,
        keycode: u32,
        key_state: KeyState,
        modifiers: Option<ModifiersState>,
        _serial: crate::utils::Serial,
        time: u32,
    ) {
        let inner = self.inner.lock().unwrap();
        let keyboard = inner.grab.as_ref().unwrap();
        inner
            .text_input_handle
            .as_ref()
            .unwrap()
            .with_focused_text_input(|_, _, serial| {
                if let Some(serialized) = modifiers.map(|m| m.serialized) {
                    keyboard.modifiers(
                        *serial,
                        serialized.depressed,
                        serialized.latched,
                        serialized.locked,
                        serialized.layout_locked,
                    )
                }
                keyboard.key(*serial, time, keycode, key_state.into());
            });
    }

    fn set_focus(
        &mut self,
        data: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        focus: Option<<D as SeatHandler>::KeyboardFocus>,
        serial: crate::utils::Serial,
    ) {
        let inner = self.inner.lock().unwrap();
        let popup = &inner.popup;
        inner
            .text_input_handle
            .as_ref()
            .unwrap()
            .set_focus(focus.as_ref().and_then(|f| f.wl_surface()), popup);
        handle.set_focus(data, focus, serial)
    }

    fn start_data(&self) -> &KeyboardGrabStartData<D> {
        &KeyboardGrabStartData { focus: None }
    }
}

/// User data of ZwpInputKeyboardGrabV2 object
#[derive(Debug)]
pub struct InputMethodKeyboardUserData<D: SeatHandler> {
    pub(super) handle: InputMethodKeyboardGrab,
    pub(crate) keyboard_handle: KeyboardHandle<D>,
}

impl<D: SeatHandler + 'static> Dispatch<ZwpInputMethodKeyboardGrabV2, InputMethodKeyboardUserData<D>, D>
    for InputMethodManagerState
{
    fn destroyed(_state: &mut D, _client: ClientId, _id: ObjectId, data: &InputMethodKeyboardUserData<D>) {
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
