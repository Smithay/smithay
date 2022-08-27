use std::sync::{Arc, Mutex};

use wayland_protocols::wp::text_input::zv3::server::zwp_text_input_v3::{self, ZwpTextInputV3};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::{protocol::wl_surface::WlSurface, Dispatch, Resource};

use crate::utils::IsAlive;
use crate::wayland::input_method::InputMethodHandle;

use super::TextInputManagerState;

#[derive(Debug)]
struct Instance {
    instance: ZwpTextInputV3,
    serial: u32,
}

#[derive(Default, Debug)]
pub(crate) struct TextInput {
    instances: Vec<Instance>,
    focus: Option<WlSurface>,
}

impl TextInput {
    fn with_focused_text_input<F>(&self, mut f: F)
    where
        F: FnMut(&ZwpTextInputV3, &WlSurface, &u32),
    {
        if let Some(ref surface) = self.focus {
            if !surface.alive() {
                return;
            }
            for ti in self.instances.iter() {
                if ti.instance.id().same_client_as(&surface.id()) {
                    f(&ti.instance, surface, &ti.serial);
                }
            }
        }
    }
}

/// Handle to text input instances
#[derive(Default, Debug, Clone)]
pub struct TextInputHandle {
    pub(crate) inner: Arc<Mutex<TextInput>>,
}

impl TextInputHandle {
    pub(super) fn add_instance<D>(&self, instance: &ZwpTextInputV3) {
        let mut inner = self.inner.lock().unwrap();
        inner.instances.push(Instance {
            instance: instance.clone(),
            serial: 0,
        });
    }

    fn increment_serial(&self, text_input: &ZwpTextInputV3) {
        let mut inner = self.inner.lock().unwrap();
        for ti in inner.instances.iter_mut() {
            if &ti.instance == text_input {
                ti.serial += 1;
            }
        }
    }

    /// Sets text input focus to a surface, the hook can be used to e.g.
    /// delete the popup surface role so it does not flicker between focused surfaces
    pub fn set_focus<F>(&self, focus: Option<&WlSurface>, focus_changed_hook: F)
    where
        F: Fn(),
    {
        let mut inner = self.inner.lock().unwrap();
        let same = inner.focus.as_ref() == focus;
        if !same {
            focus_changed_hook();
            inner.with_focused_text_input(|ti, surface, _serial| {
                ti.leave(surface);
            });

            inner.focus = focus.cloned();

            inner.with_focused_text_input(|ti, surface, _serial| {
                ti.enter(surface);
            });
        }
    }

    /// Callback function to use on the current focused text input surface
    pub fn with_focused_text_input<F>(&self, f: F)
    where
        F: FnMut(&ZwpTextInputV3, &WlSurface, &u32),
    {
        let inner = self.inner.lock().unwrap();
        inner.with_focused_text_input(f);
    }
}

/// User data of ZwpTextInputV3 object
#[derive(Debug)]
pub struct TextInputUserData {
    pub(super) handle: TextInputHandle,
    pub(crate) input_method_handle: InputMethodHandle,
}

impl<D> Dispatch<ZwpTextInputV3, TextInputUserData, D> for TextInputManagerState
where
    D: Dispatch<ZwpTextInputV3, TextInputUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        resource: &ZwpTextInputV3,
        request: zwp_text_input_v3::Request,
        data: &TextInputUserData,
        _dhandle: &wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_text_input_v3::Request::Enable => {
                data.input_method_handle
                    .with_instance(|input_method| input_method.activate());
            }
            zwp_text_input_v3::Request::Disable => {
                data.input_method_handle
                    .with_instance(|input_method| input_method.deactivate());
            }
            zwp_text_input_v3::Request::SetSurroundingText { text, cursor, anchor } => {
                data.input_method_handle.with_instance(|input_method| {
                    input_method.surrounding_text(text.clone(), cursor as u32, anchor as u32)
                });
            }
            zwp_text_input_v3::Request::SetTextChangeCause { cause } => {
                data.input_method_handle.with_instance(|input_method| {
                    input_method.text_change_cause(cause.into_result().unwrap())
                });
            }
            zwp_text_input_v3::Request::SetContentType { hint, purpose } => {
                data.input_method_handle.with_instance(|input_method| {
                    input_method.content_type(hint.into_result().unwrap(), purpose.into_result().unwrap());
                });
            }
            zwp_text_input_v3::Request::SetCursorRectangle { x, y, width, height } => {
                let input_method = data.input_method_handle.inner.lock().unwrap();
                input_method.popup.add_coordinates(x, y, width, height);
                let popup_surface = &input_method.popup.inner.lock().unwrap();
                if let Some(popup) = &popup_surface.surface_role {
                    popup.text_input_rectangle(x, y, width, height);
                }
            }
            zwp_text_input_v3::Request::Commit => {
                data.handle.increment_serial(resource);
                data.input_method_handle.with_instance(|input_method| {
                    input_method.done();
                });
            }
            zwp_text_input_v3::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(_state: &mut D, _client: ClientId, ti: ObjectId, data: &TextInputUserData) {
        data.handle
            .inner
            .lock()
            .unwrap()
            .instances
            .retain(|i| i.instance.id() != ti);
    }
}
