use std::sync::{Arc, Mutex};

use wayland_backend::server::{ClientId, ObjectId};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_v1::{self, ZwpVirtualKeyboardV1};
use wayland_server::{Dispatch, Client, DisplayHandle, DataInit};
use xkbcommon::xkb::{KeymapFormat, self};

use crate::{wayland::{seat::{KeyboardHandle, Seat, FilterResult}, SERIAL_COUNTER}, backend::input::KeyState};

use super::VirtualKeyboardManagerState;

#[derive(Default, Debug)]
pub(crate) struct VirtualKeyboard {
    pub instance : Option<ZwpVirtualKeyboardV1>,
    modifiers: Option<(u32, u32, u32, u32)>,
    keyboard_handle: Option<KeyboardHandle>,
}

/// Handle to a virtual keyboard instance
#[derive(Default, Debug, Clone)]
pub struct VirtualKeyboardHandle {
    pub(crate) inner: Arc<Mutex<VirtualKeyboard>>,
}

impl VirtualKeyboardHandle {
    pub(super) fn add_instance<D>(&self, instance: &ZwpVirtualKeyboardV1) {
        let mut inner = self.inner.lock().unwrap();
        inner.instance = Some(instance.clone());
    }

    pub(super) fn has_instance(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.instance.is_some()
    }
}

/// User data of ZwpVirtualKeyboardV1 object
#[derive(Debug)]
pub struct VirtualKeyboardUserData<D> {
    pub(super) handle: VirtualKeyboardHandle,
    pub(super) seat: Seat<D>,
}

impl<D> Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData<D>, D> for VirtualKeyboardManagerState
where
    D: Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData<D>>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _: &ZwpVirtualKeyboardV1,
        request: zwp_virtual_keyboard_v1::Request,
        data: &VirtualKeyboardUserData<D>,
        dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_virtual_keyboard_v1::Request::Keymap { format, fd, size } => {
                let mut inner = data.handle.inner.lock().unwrap();
                let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                let keymap = xkb::Keymap::new_from_fd(&context, fd, size as usize, format, xkb::KEYMAP_COMPILE_NO_FLAGS).unwrap();
                
                let keyboard = data.seat.add_keyboard(xkb_config, 200, 25, |_, _| {}).unwrap();
                inner.keyboard_handle.replace(keyboard);
            },
            zwp_virtual_keyboard_v1::Request::Key { time, key, state } => {
                let mut inner = data.handle.inner.lock().unwrap();
                if let Some(keyboard) = inner.keyboard_handle {
                    let key_state = if state == 1 {
                        KeyState::Pressed
                    } else {
                        KeyState::Released
                    };
                    let modifiers = data.handle.inner.lock().unwrap().modifiers;
                    keyboard.input(dh, key, key_state, SERIAL_COUNTER.next_serial(), time, |_, _| FilterResult::Forward);
                }
            },
            zwp_virtual_keyboard_v1::Request::Modifiers { mods_depressed, mods_latched, mods_locked, group } => {
                data.handle.inner.lock().unwrap().modifiers = Some((mods_depressed, mods_latched, mods_locked, group));
            },
            zwp_virtual_keyboard_v1::Request::Destroy => {
                // Nothing to do
            },
            _ => todo!(),
        }
    }

    fn destroyed(_state: &mut D, _client: ClientId, _virtual_keyboard: ObjectId, data: &VirtualKeyboardUserData<D>) {
        data.handle.inner.lock().unwrap().instance = None;
    }
}