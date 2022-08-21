use std::sync::{Arc, Mutex};

use wayland_backend::server::{ClientId, ObjectId};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_v1::{self, ZwpVirtualKeyboardV1};
use wayland_server::{Dispatch, Client, DisplayHandle, DataInit};

use crate::wayland::seat::{KeyboardGrab, KeyboardGrabStartData, KeyboardInnerHandle, KeyboardHandle};

use super::VirtualKeyboardManagerState;

#[derive(Default, Debug)]
pub(crate) struct VirtualKeyboard {
    pub instance : Option<ZwpVirtualKeyboardV1>,
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

impl KeyboardGrab for VirtualKeyboardHandle {
    fn input(
        &mut self,
        _dh: &DisplayHandle,
        handle: &mut KeyboardInnerHandle<'_>,
        keycode: u32,
        key_state: wayland_server::protocol::wl_keyboard::KeyState,
        modifiers: Option<(u32, u32, u32, u32)>,
        serial: crate::wayland::Serial,
        time: u32,
    ) {
        handle.input(keycode, key_state, modifiers, serial, time);
    }

    fn set_focus(
        &mut self,
        _dh: &DisplayHandle,
        handle: &mut KeyboardInnerHandle<'_>,
        focus: Option<&wayland_server::protocol::wl_surface::WlSurface>,
        serial: crate::wayland::Serial,
    ) {
        handle.set_focus(focus, serial);
    }

    fn start_data(&self) -> &KeyboardGrabStartData {
        &KeyboardGrabStartData { focus: None }
    }
}

/// User data of ZwpVirtualKeyboardV1 object
#[derive(Debug)]
pub struct VirtualKeyboardUserData {
    pub(super) handle: VirtualKeyboardHandle,
    pub(crate) keyboard_handle: KeyboardHandle,
    modifiers: Option<(u32, u32, u32, u32)>,
}

impl<D> Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData, D> for VirtualKeyboardManagerState
where
    D: Dispatch<ZwpVirtualKeyboardV1, VirtualKeyboardUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _seat: &ZwpVirtualKeyboardV1,
        request: zwp_virtual_keyboard_v1::Request,
        data: &VirtualKeyboardUserData,
        dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_virtual_keyboard_v1::Request::Keymap { format, fd, size } => {
                data.keyboard_handle.
            },
            zwp_virtual_keyboard_v1::Request::Key { time, key, state } => {
                data.handle.input(dh, data.handle, key, state.into(), data.modifiers, serial, time)
            },
            zwp_virtual_keyboard_v1::Request::Modifiers { mods_depressed, mods_latched, mods_locked, group } => {
                data.modifiers = Some((mods_depressed, mods_latched, mods_locked, group));
            },
            zwp_virtual_keyboard_v1::Request::Destroy => {
                // Nothing to do
            },
            _ => todo!(),
        }
    }

    fn destroyed(_state: &mut D, _client: ClientId, _virtual_keyboard: ObjectId, data: &VirtualKeyboardUserData) {
        data.handle.inner.lock().unwrap().instance = None;
    }
}