use std::sync::{Arc, Mutex};

use tracing::debug;
use wayland_protocols::wp::text_input::zv3::server::zwp_text_input_v3::{self, ZwpTextInputV3};
use wayland_server::backend::ClientId;
use wayland_server::{protocol::wl_surface::WlSurface, Dispatch, Resource};

use crate::input::SeatHandler;
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
    fn with_focused_text_input<F>(&mut self, mut f: F)
    where
        F: FnMut(&ZwpTextInputV3, &WlSurface, u32),
    {
        if let Some(ref surface) = self.focus {
            if !surface.alive() {
                return;
            }
            for ti in self.instances.iter_mut() {
                if ti.instance.id().same_client_as(&surface.id()) {
                    f(&ti.instance, surface, ti.serial);
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
    pub(super) fn add_instance(&self, instance: &ZwpTextInputV3) {
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

    /// Return the currently focused surface.
    pub fn focus(&self) -> Option<WlSurface> {
        self.inner.lock().unwrap().focus.clone()
    }

    /// Advance the focus for the client to `surface`.
    ///
    /// This doesn't send any 'enter' or 'leave' events.
    pub fn set_focus(&self, surface: Option<WlSurface>) {
        self.inner.lock().unwrap().focus = surface;
    }

    /// Send `leave` on the text-input instance for the currently focused
    /// surface.
    pub fn leave(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.with_focused_text_input(|text_input, focus, _| {
            text_input.leave(focus);
        });
    }

    /// Send `enter` on the text-input instance for the currently focused
    /// surface.
    pub fn enter(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.with_focused_text_input(|text_input, focus, _| {
            text_input.enter(focus);
        });
    }

    /// The `discard_state` is used when the input-method signaled that
    /// the state should be discarded and wrong serial sent.
    pub fn done(&self, discard_state: bool) {
        let mut inner = self.inner.lock().unwrap();
        inner.with_focused_text_input(|text_input, _, serial| {
            if discard_state {
                debug!("discarding text-input state due to serial");
                // Discarding is done by sending non-matching serial.
                text_input.done(0);
            } else {
                text_input.done(serial);
            }
        });
    }

    /// Access the text-input instance for the currently focused surface.
    pub fn with_focused_text_input<F>(&self, mut f: F)
    where
        F: FnMut(&ZwpTextInputV3, &WlSurface),
    {
        let mut inner = self.inner.lock().unwrap();
        inner.with_focused_text_input(|ti, surface, _| {
            f(ti, surface);
        });
    }

    /// Call the callback with the serial of the focused text_input or with the passed
    /// `default` one when empty.
    pub(crate) fn focused_text_input_serial_or_default<F>(&self, default: u32, mut callback: F)
    where
        F: FnMut(u32),
    {
        let mut inner = self.inner.lock().unwrap();
        let mut should_default = true;
        inner.with_focused_text_input(|_, _, serial| {
            should_default = false;
            callback(serial);
        });
        if should_default {
            callback(default)
        }
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
    D: SeatHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        resource: &ZwpTextInputV3,
        request: zwp_text_input_v3::Request,
        data: &TextInputUserData,
        _dhandle: &wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        // Always increment serial to not desync with clients.
        if matches!(request, zwp_text_input_v3::Request::Commit) {
            data.handle.increment_serial(resource);
        }

        // Discard requsets without any active input method instance.
        if !data.input_method_handle.has_instance() {
            debug!("discarding text-input request without IME running");
            return;
        }

        let focus = match data.handle.focus() {
            Some(focus) if focus.id().same_client_as(&resource.id()) => focus,
            _ => {
                debug!("discarding text-input request for unfocused client");
                return;
            }
        };

        match request {
            zwp_text_input_v3::Request::Enable => {
                data.input_method_handle.activate_input_method(state, &focus)
            }
            zwp_text_input_v3::Request::Disable => {
                data.input_method_handle.deactivate_input_method(state, false);
            }
            zwp_text_input_v3::Request::SetSurroundingText { text, cursor, anchor } => {
                data.input_method_handle.with_instance(|input_method| {
                    input_method
                        .object
                        .surrounding_text(text.clone(), cursor as u32, anchor as u32)
                });
            }
            zwp_text_input_v3::Request::SetTextChangeCause { cause } => {
                data.input_method_handle.with_instance(|input_method| {
                    input_method
                        .object
                        .text_change_cause(cause.into_result().unwrap())
                });
            }
            zwp_text_input_v3::Request::SetContentType { hint, purpose } => {
                data.input_method_handle.with_instance(|input_method| {
                    input_method
                        .object
                        .content_type(hint.into_result().unwrap(), purpose.into_result().unwrap());
                });
            }
            zwp_text_input_v3::Request::SetCursorRectangle { x, y, width, height } => {
                data.input_method_handle
                    .set_text_input_rectangle(x, y, width, height);
            }
            zwp_text_input_v3::Request::Commit => {
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

    fn destroyed(state: &mut D, _client: ClientId, text_input: &ZwpTextInputV3, data: &TextInputUserData) {
        let destroyed_id = text_input.id();
        let deactivate_im = {
            let mut inner = data.handle.inner.lock().unwrap();
            inner.instances.retain(|inst| inst.instance.id() != destroyed_id);
            let destroyed_focused = inner
                .focus
                .as_ref()
                .map(|focus| focus.id().same_client_as(&destroyed_id))
                .unwrap_or(true);

            // Deactivate IM when we either lost focus entirely or destroyed text-input for the
            // currently focused client.
            destroyed_focused
                && !inner
                    .instances
                    .iter()
                    .any(|inst| inst.instance.id().same_client_as(&destroyed_id))
        };

        if deactivate_im {
            data.input_method_handle.deactivate_input_method(state, true);
        }
    }
}
