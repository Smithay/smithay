use std::{
    fmt,
    sync::{Arc, Mutex},
};

use wayland_protocols_experimental::input_method::v1::server::{
    xx_input_method_v1::{self, XxInputMethodV1},
    xx_input_popup_surface_v2::XxInputPopupSurfaceV2,
};
use wayland_server::{backend::ClientId, protocol::wl_surface::WlSurface};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, Resource};

use crate::{
    input::SeatHandler,
    utils::{Logical, Rectangle},
    wayland::{compositor, seat::WaylandFocus, text_input::TextInputHandle},
};

use super::{
    input_method_popup_surface::{ImPopupLocation, PopupParent, PopupSurface},
    positioner::{PositionerState, PositionerUserData},
    InputMethodHandler, InputMethodManagerState, InputMethodPopupSurfaceUserData, INPUT_POPUP_SURFACE_ROLE,
};

/// Slot for an optional input method
#[derive(Default, Debug)]
pub(crate) struct MaybeInstance {
    /// Optional input method
    pub instance: Option<InputMethod>,
}

/// Contains input method state
#[derive(Debug)]
pub(crate) struct InputMethod {
    pub object: XxInputMethodV1,
    pub serial: u32,
    pub active: bool,
    pub popup_handles: Vec<PopupSurface>,
    /// Relative to surface on which input method is enabled
    pub cursor_rectangle: Rectangle<i32, Logical>,
}

impl InputMethod {
    /// Send the done incrementing the serial.
    pub(crate) fn done(&mut self) {
        self.object.done();
        self.serial += 1;
    }
}

/// Handle to a possible input method instance.
#[derive(Default, Debug, Clone)]
pub struct InputMethodHandle {
    // TODO: why does this need to be shared?
    pub(crate) inner: Arc<Mutex<MaybeInstance>>,
}

impl InputMethodHandle {
    /// Assigns a new instance
    pub(super) fn add_instance(&self, instance: &XxInputMethodV1) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(instance) = inner.instance.as_mut() {
            instance.serial = 0;
            instance.object.unavailable();
        } else {
            inner.instance = Some(InputMethod {
                object: instance.clone(),
                serial: 0,
                active: false,
                popup_handles: vec![],
                cursor_rectangle: Rectangle::default(),
            });
        }
    }

    /// Whether there's an active instance of input-method.
    pub(crate) fn has_instance(&self) -> bool {
        self.inner.lock().unwrap().instance.is_some()
    }

    /// Callback function to access the input method object
    pub(crate) fn with_instance<F>(&self, f: F)
    where
        F: FnOnce(&mut InputMethod),
    {
        let mut inner = self.inner.lock().unwrap();
        if let Some(instance) = inner.instance.as_mut() {
            f(instance);
        }
    }

    pub(crate) fn set_cursor_rectangle<D: SeatHandler + 'static>(
        &self,
        state: &mut D,
        cursor: Rectangle<i32, Logical>,
    ) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(ref mut inner) = &mut inner.instance {
            let data = inner.object.data::<InputMethodUserData<D>>().unwrap();
            inner.cursor_rectangle = cursor;
            for popup_surface in &mut inner.popup_handles {
                let popup_geometry = (data.popup_geometry)(
                    state,
                    &popup_surface.get_parent().surface,
                    &cursor,
                    &popup_surface.positioner(),
                );

                let anchor = cursor; // FIXME: choose the anchor which the positioner wants

                popup_surface.set_position(ImPopupLocation {
                    anchor,
                    geometry: popup_geometry,
                });

                // TODO: send now or on .done?
                (data.popup_repositioned)(state, popup_surface.clone());
            }
        }
    }

    pub(crate) fn done(&self) {
        let mut inner = self.inner.lock().unwrap();

        if let Some(ref mut inner) = &mut inner.instance {
            for popup_surface in &mut inner.popup_handles {
                popup_surface.send_pending_configure();
            }
            inner.done();
        }
    }

    /// Activate input method on the given surface.
    pub(crate) fn activate_input_method<D: SeatHandler + 'static>(&self, _: &mut D, _surface: &WlSurface) {
        self.with_instance(|im| {
            im.object.activate();
            im.active = true;
        });
    }

    /// Deactivate the active input method.
    ///
    /// This includes a complete sequence including .done.
    pub(crate) fn deactivate_input_method<D: SeatHandler + 'static>(&self, state: &mut D) {
        self.with_instance(|im| {
            im.object.deactivate();
            im.done();
            im.active = false;
            let data = im.object.data::<InputMethodUserData<D>>().unwrap();
            for popup in im.popup_handles.drain(..) {
                (data.dismiss_popup)(state, popup.clone());
            }
        });
    }
}

/// User data of XxInputMethodV1 object
#[derive(Clone)]
pub struct InputMethodUserData<D: SeatHandler> {
    pub(super) handle: InputMethodHandle,
    pub(crate) text_input_handle: TextInputHandle,
    /// This is just a copy from Input MethodHandler. It's here in order to break the requirement for D: InputMethodHandler on functions that call dismiss_popup. That means other modules don't have to explicitly put D: InputMethodHandler when they call something that ends up calling this.
    /// (Not sure what the purpose of that is, but it seems consistent...)
    pub(crate) popup_geometry:
        fn(&D, &WlSurface, &Rectangle<i32, Logical>, &PositionerState) -> Rectangle<i32, Logical>,
    pub(crate) popup_repositioned: fn(&mut D, PopupSurface),
    pub(crate) dismiss_popup: fn(&mut D, PopupSurface),
}

impl<D: SeatHandler> fmt::Debug for InputMethodUserData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputMethodUserData")
            .field("handle", &self.handle)
            .field("text_input_handle", &self.text_input_handle)
            .finish()
    }
}

impl<D> Dispatch<XxInputMethodV1, InputMethodUserData<D>, D> for InputMethodManagerState
where
    D: Dispatch<XxInputMethodV1, InputMethodUserData<D>>,
    D: Dispatch<XxInputPopupSurfaceV2, InputMethodPopupSurfaceUserData>,
    D: SeatHandler,
    D: InputMethodHandler,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        im: &XxInputMethodV1,
        request: xx_input_method_v1::Request,
        data: &InputMethodUserData<D>,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        use xx_input_method_v1::Request;
        match request {
            Request::CommitString { text } => {
                data.text_input_handle.with_active_text_input(|ti, _surface| {
                    ti.commit_string(Some(text.clone()));
                });
            }
            Request::SetPreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                data.text_input_handle.with_active_text_input(|ti, _surface| {
                    ti.preedit_string(Some(text.clone()), cursor_begin, cursor_end);
                });
            }
            Request::DeleteSurroundingText {
                before_length,
                after_length,
            } => {
                data.text_input_handle.with_active_text_input(|ti, _surface| {
                    ti.delete_surrounding_text(before_length, after_length);
                });
            }
            Request::Commit { serial } => {
                let current_serial = data
                    .handle
                    .inner
                    .lock()
                    .unwrap()
                    .instance
                    .as_ref()
                    .map(|i| i.serial)
                    .unwrap_or(0);

                data.text_input_handle.done(serial != current_serial);
            }
            Request::GetInputPopupSurface {
                id,
                surface,
                positioner,
            } => {
                let mut input_method = data.handle.inner.lock().unwrap();
                if let Some(instance) = &mut input_method.instance {
                    if instance.active {
                        if compositor::give_role(&surface, INPUT_POPUP_SURFACE_ROLE).is_err()
                            && compositor::get_role(&surface) != Some(INPUT_POPUP_SURFACE_ROLE)
                        {
                            im.post_error(
                                xx_input_method_v1::Error::SurfaceHasRole,
                                "Surface already has a role.",
                            );
                            return;
                        }

                        let parent_surface = match data.text_input_handle.focus().clone() {
                            Some(parent) => parent,
                            None => {
                                im.post_error(
                                    xx_input_method_v1::Error::Inactive,
                                    "Popup may only be created on an active input method (no surface in text input focus).",
                                );
                                return;
                            }
                        };

                        let location = state.parent_geometry(&parent_surface);
                        let parent = PopupParent {
                            surface: parent_surface,
                            location,
                        };

                        let positioner_data = *positioner
                            .data::<PositionerUserData>()
                            .unwrap()
                            .inner
                            .lock()
                            .unwrap();

                        let geometry = state.popup_geometry(
                            &parent.surface,
                            &instance.cursor_rectangle,
                            &positioner_data,
                        );

                        // TODO: feed the popup with the anchor chosen by the positioner
                        let popup = PopupSurface::new(
                            |data| data_init.init(id, data),
                            im.clone(),
                            parent,
                            surface,
                            instance.cursor_rectangle,
                            geometry,
                            positioner_data,
                        );
                        instance.popup_handles.push(popup.clone());
                        state.new_popup(popup);
                    } else {
                        im.post_error(
                            xx_input_method_v1::Error::Inactive,
                            "Popup may only be created on an active input method.",
                        );
                    }
                }
            }
            Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _client: ClientId,
        _input_method: &XxInputMethodV1,
        data: &InputMethodUserData<D>,
    ) {
        data.handle.inner.lock().unwrap().instance = None;
        data.text_input_handle.leave();
    }
}
