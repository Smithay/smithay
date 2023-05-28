use std::{
    fmt,
    sync::{Arc, Mutex},
};

use wayland_protocols_misc::zwp_input_method_v2::server::{
    zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2,
    zwp_input_method_v2::{self, ZwpInputMethodV2},
    zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2,
};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::{
    protocol::{wl_keyboard::KeymapFormat, wl_surface::WlSurface},
    Client, DataInit, Dispatch, DisplayHandle,
};

use crate::{
    input::{keyboard::KeyboardHandle, SeatHandler},
    utils::{IsAlive, Logical, Physical, Point, Rectangle, SERIAL_COUNTER},
    wayland::{seat::WaylandFocus, text_input::TextInputHandle},
};

use super::{
    input_method_keyboard_grab::InputMethodKeyboardGrab,
    input_method_popup_surface::InputMethodPopupSurfaceHandle, InputMethodKeyboardUserData,
    InputMethodManagerState, InputMethodPopupSurfaceUserData,
};

#[derive(Default, Debug)]
pub(crate) struct InputMethod {
    pub instance: Option<ZwpInputMethodV2>,
    pub popup: InputMethodPopupSurfaceHandle,
    pub keyboard_grab: InputMethodKeyboardGrab,
}

/// Handle to an input method instance
#[derive(Default, Debug, Clone)]
pub struct InputMethodHandle {
    pub(crate) inner: Arc<Mutex<InputMethod>>,
}

impl InputMethodHandle {
    pub(super) fn add_instance<D>(&self, instance: &ZwpInputMethodV2) {
        let mut inner = self.inner.lock().unwrap();
        if inner.instance.is_some() {
            instance.unavailable()
        } else {
            inner.instance = Some(instance.clone());
        }
    }

    /// Callback function to access the input method object
    pub fn with_instance<F>(&self, mut f: F)
    where
        F: FnMut(&ZwpInputMethodV2),
    {
        let inner = self.inner.lock().unwrap();
        if let Some(instance) = &inner.instance {
            f(instance);
        }
    }

    /// Indicates that an input method has grabbed a keyboard
    pub fn keyboard_grabbed(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        let keyboard = inner.keyboard_grab.inner.lock().unwrap();
        keyboard.grab.is_some()
    }

    /// Convenience function to draw surfaces
    pub fn with_surface<F>(&self, mut f: F)
    where
        F: FnMut(&WlSurface),
    {
        let inner = self.inner.lock().unwrap();
        let popup = inner.popup.inner.lock().unwrap();
        if popup.surface_role.is_some() {
            if let Some(surface) = &popup.surface {
                if surface.alive() {
                    f(surface);
                }
            }
        }
    }

    /// Convenience function to close popup surfaces
    pub(crate) fn close_popup(&self) {
        let inner = self.inner.lock().unwrap();
        let mut popup = inner.popup.inner.lock().unwrap();
        popup.surface_role = None;
    }

    /// Used to access the relative location of an input popup surface
    pub fn coordinates(&self) -> Rectangle<i32, Physical> {
        let inner = self.inner.lock().unwrap();
        inner.popup.coordinates()
    }

    /// Sets the point of the upper left corner of the surface in focus
    pub fn set_point(&self, point: &Point<i32, Logical>) {
        let mut inner = self.inner.lock().unwrap();
        inner.popup.set_point(point);
    }
}

/// User data of ZwpInputMethodV2 object
pub struct InputMethodUserData<D: SeatHandler> {
    pub(super) handle: InputMethodHandle,
    pub(crate) text_input_handle: TextInputHandle,
    pub(crate) keyboard_handle: KeyboardHandle<D>,
}

impl<D: SeatHandler> fmt::Debug for InputMethodUserData<D>
where
    <D as SeatHandler>::KeyboardFocus: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputMethodUserData")
            .field("handle", &self.handle)
            .field("text_input_handle", &self.text_input_handle)
            .field("keyboard_handle", &self.keyboard_handle)
            .finish()
    }
}

impl<D> Dispatch<ZwpInputMethodV2, InputMethodUserData<D>, D> for InputMethodManagerState
where
    D: Dispatch<ZwpInputMethodV2, InputMethodUserData<D>>,
    D: Dispatch<ZwpInputPopupSurfaceV2, InputMethodPopupSurfaceUserData>,
    D: Dispatch<ZwpInputMethodKeyboardGrabV2, InputMethodKeyboardUserData<D>>,
    D: SeatHandler,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _seat: &ZwpInputMethodV2,
        request: zwp_input_method_v2::Request,
        data: &InputMethodUserData<D>,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_input_method_v2::Request::CommitString { text } => {
                data.text_input_handle.with_focused_text_input(|ti, _surface, _| {
                    ti.commit_string(Some(text.clone()));
                });
            }
            zwp_input_method_v2::Request::SetPreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                data.text_input_handle.with_focused_text_input(|ti, _surface, _| {
                    ti.preedit_string(Some(text.clone()), cursor_begin, cursor_end);
                });
            }
            zwp_input_method_v2::Request::DeleteSurroundingText {
                before_length,
                after_length,
            } => {
                data.text_input_handle.with_focused_text_input(|ti, _surface, _| {
                    ti.delete_surrounding_text(before_length, after_length);
                });
            }
            zwp_input_method_v2::Request::Commit { serial } => {
                data.text_input_handle.with_focused_text_input(|ti, _surface, _| {
                    ti.done(serial);
                });
            }
            zwp_input_method_v2::Request::GetInputPopupSurface { id, surface } => {
                let input_method = data.handle.inner.lock().unwrap();
                let instance = data_init.init(
                    id,
                    InputMethodPopupSurfaceUserData {
                        handle: input_method.popup.clone(),
                    },
                );
                let mut popup = input_method.popup.inner.lock().unwrap();
                popup.surface_role = Some(instance);
                popup.surface = Some(surface);
            }
            zwp_input_method_v2::Request::GrabKeyboard { keyboard } => {
                let input_method = data.handle.inner.lock().unwrap();
                data.keyboard_handle
                    .set_grab(input_method.keyboard_grab.clone(), SERIAL_COUNTER.next_serial());
                let instance = data_init.init(
                    keyboard,
                    InputMethodKeyboardUserData {
                        handle: input_method.keyboard_grab.clone(),
                        keyboard_handle: data.keyboard_handle.clone(),
                    },
                );
                let mut keyboard = input_method.keyboard_grab.inner.lock().unwrap();
                keyboard.grab = Some(instance.clone());
                keyboard.text_input_handle = data.text_input_handle.clone();
                keyboard.popup_handle = input_method.popup.clone();
                let guard = data.keyboard_handle.arc.internal.lock().unwrap();
                instance.repeat_info(guard.repeat_rate, guard.repeat_delay);
                let keymap_file = data.keyboard_handle.arc.keymap.lock().unwrap();
                keymap_file
                    .with_fd(false, |fd, size| {
                        instance.keymap(KeymapFormat::XkbV1, fd, size as u32);
                    })
                    .unwrap(); //TODO: log some kind of error here
            }
            zwp_input_method_v2::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(_state: &mut D, _client: ClientId, _input_method: ObjectId, data: &InputMethodUserData<D>) {
        data.handle.inner.lock().unwrap().instance = None;
        data.text_input_handle.with_focused_text_input(|ti, surface, _| {
            ti.leave(surface);
        });
    }
}
