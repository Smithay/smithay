use std::sync::{Arc, Mutex};

use tracing::debug;
use wayland_protocols::wp::text_input::zv3::server::zwp_text_input_v3::{self, ZwpTextInputV3, ContentHint, ContentPurpose};
use wayland_protocols_plasma::text_input::v2::server::zwp_text_input_v2::{ZwpTextInputV2, self, UpdateState};
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

#[derive(Debug)]
struct InstanceV2 {
    instance: ZwpTextInputV2,
    serial: u32,
}

#[derive(Default, Debug)]
pub(crate) struct TextInput {
    instances: Vec<Instance>,
    instances_v2: Vec<InstanceV2>,
    focus: Option<WlSurface>,
}

impl TextInput {
    fn with_focused_text_input<F>(&mut self, mut f: F)
    where
        F: FnMut(Option<&ZwpTextInputV3>, Option<&ZwpTextInputV2>, &WlSurface, u32),
    {
        if let Some(ref surface) = self.focus {
            if !surface.alive() {
                return;
            }
            for (ti, ti_v2) in self.instances.iter_mut().zip(self.instances_v2.iter_mut()) {
                if ti.instance.id().same_client_as(&surface.id()) {
                    f(Some(&ti.instance), None, surface, ti.serial);
                } else if ti_v2.instance.id().same_client_as(&surface.id()) {
                    f(None, Some(&ti_v2.instance), surface, ti_v2.serial);
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
        inner.with_focused_text_input(|text_input, text_input_v2, focus, serial| {
            if let Some(text_input) = text_input {
                text_input.leave(focus);
            } else if let Some(text_input) = text_input_v2 {
                text_input.leave(serial, focus);
            }
        });
    }

    /// Send `enter` on the text-input instance for the currently focused
    /// surface.
    pub fn enter(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.with_focused_text_input(|text_input, text_input_v2, focus, serial| {
            if let Some(text_input) = text_input {
                text_input.enter(focus);
            } else if let Some(text_input) = text_input_v2 {
                text_input.enter(serial, focus);
            }
        });
    }

    /// The `discard_state` is used when the input-method signaled that
    /// the state should be discarded and wrong serial sent.
    pub fn done(&self, discard_state: bool) {
        let mut inner = self.inner.lock().unwrap();
        inner.with_focused_text_input(|text_input, text_input_v2, focus, serial| {
            if let Some(text_input) = text_input {
                if discard_state {
                    debug!("discarding text-input state due to serial");
                    // Discarding is done by sending non-matching serial.
                    text_input.done(0);
                } else {
                    text_input.done(serial);
                }
            } else if let Some(text_input) = text_input_v2 {
                if discard_state {
                    debug!("discarding text-input state due to serial");
                    // Discarding is done by sending non-matching serial.
                    text_input.input_method_changed(0, UpdateState::Reset.into());
                }
            }
        });
    }

    /// Access the text-input instance for the currently focused surface.
    pub fn with_focused_text_input<F>(&self, mut f: F)
    where
        F: FnMut(Option<&ZwpTextInputV3>, Option<&ZwpTextInputV2>, &WlSurface, u32),
    {
        let mut inner = self.inner.lock().unwrap();
        inner.with_focused_text_input(|ti, ti_v2, surface, serial| {
            f(ti, ti_v2, surface, serial);
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
        inner.with_focused_text_input(|_, _, _, serial| {
            should_default = false;
            callback(serial);
        });
        if should_default {
            callback(default)
        }
    }

    pub(crate) fn add_instance_v2(&self, instance: &ZwpTextInputV2) {
        let mut inner = self.inner.lock().unwrap();
        inner.instances_v2.push(InstanceV2 {
            instance: instance.clone(),
            serial: 0,
        });
    }

    fn set_serial_v2(&self, text_input: &ZwpTextInputV2, serial: u32) {
        let mut inner = self.inner.lock().unwrap();
        for ti in inner.instances_v2.iter_mut() {
            if &ti.instance == text_input {
                ti.serial = serial;
            }
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

impl<D> Dispatch<ZwpTextInputV2, TextInputUserData, D> for TextInputManagerState
where
    D: Dispatch<ZwpTextInputV2, TextInputUserData>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        resource: &ZwpTextInputV2,
        request: zwp_text_input_v2::Request,
        data: &TextInputUserData,
        _dhandle: &wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
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
            zwp_text_input_v2::Request::Enable{ surface:_ } => {
                data.input_method_handle.activate_input_method(state, &focus)
            }
            zwp_text_input_v2::Request::Disable { surface:_ } => {
                data.input_method_handle.deactivate_input_method(state, false);
            }
            zwp_text_input_v2::Request::SetSurroundingText { text, cursor, anchor } => {
                data.input_method_handle.with_instance(|input_method| {
                    input_method
                        .object
                        .surrounding_text(text.clone(), cursor as u32, anchor as u32)
                });
            }
            // zwp_text_input_v2::Request::SetContentType { hint, purpose } => {
            //     let n = ContentHint::from_bits(hint.into_result().unwrap().bits());
            //     data.input_method_handle.with_instance(|input_method| {
            //         input_method
            //             .object
            //             .content_type(
            //                 ContentHint::from_bits(hint.into_result().unwrap().bits()).unwrap(),
            //                 ContentPurpose::try_from(u32::from(purpose.into_result().unwrap())).unwrap()
            //             );
            //     });
            // }
            zwp_text_input_v2::Request::SetCursorRectangle { x, y, width, height } => {
                data.input_method_handle
                    .set_text_input_rectangle(x, y, width, height);
            }
            zwp_text_input_v2::Request::UpdateState { serial, reason } => {
                data.handle.set_serial_v2(resource, serial);
                data.input_method_handle.with_instance(|input_method| {
                    input_method.done();
                });
            }
            zwp_text_input_v2::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(state: &mut D, _client: ClientId, text_input: &ZwpTextInputV2, data: &TextInputUserData) {
        let destroyed_id = text_input.id();
        let deactivate_im = {
            let mut inner = data.handle.inner.lock().unwrap();
            inner.instances_v2.retain(|inst| inst.instance.id() != destroyed_id);
            let destroyed_focused = inner
                .focus
                .as_ref()
                .map(|focus| focus.id().same_client_as(&destroyed_id))
                .unwrap_or(true);

            // Deactivate IM when we either lost focus entirely or destroyed text-input for the
            // currently focused client.
            destroyed_focused
                && !inner
                    .instances_v2
                    .iter()
                    .any(|inst| inst.instance.id().same_client_as(&destroyed_id))
        };

        if deactivate_im {
            data.input_method_handle.deactivate_input_method(state, true);
        }
    }
}