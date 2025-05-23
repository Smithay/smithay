use std::{
    fmt,
    sync::{Arc, Mutex},
};

use tracing::warn;
use wayland_server::{backend::ClientId, protocol::wl_surface::WlSurface};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, Resource};
use wl_input_method::input_method::xx::server::{
    xx_input_method_v1::{self, XxInputMethodV1},
    xx_input_popup_surface_v2::XxInputPopupSurfaceV2,
};

use crate::{
    input::SeatHandler,
    utils::{alive_tracker::AliveTracker, Logical, Rectangle},
    wayland::{compositor, seat::WaylandFocus, text_input::TextInputHandle},
};

use super::{
    input_method_popup_surface::{ImPopupLocation, PopupParent, PopupSurface, PopupSurfaceState},
    positioner::{PositionerState, PositionerUserData},
    InputMethodHandler, InputMethodManagerState, InputMethodPopupSurfaceUserData, INPUT_POPUP_SURFACE_ROLE,
};

#[derive(Default, Debug)]
pub(crate) struct InputMethod {
    pub instance: Option<Instance>,
    pub popup_handles: Vec<PopupSurface>,
    /// Relative to surface on which input method is enabled
    pub cursor_rectangle: Rectangle<i32, Logical>,
}

#[derive(Debug)]
pub(crate) struct Instance {
    pub object: XxInputMethodV1,
    pub serial: u32,
    pub active: bool,
}

impl Instance {
    /// Send the done incrementing the serial.
    pub(crate) fn done(&mut self) {
        self.object.done();
        self.serial += 1;
    }
}

/// Handle to an input method instance
#[derive(Default, Debug, Clone)]
pub struct InputMethodHandle {
    pub(crate) inner: Arc<Mutex<InputMethod>>,
}

impl InputMethodHandle {
    pub(super) fn add_instance(&self, instance: &XxInputMethodV1) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(instance) = inner.instance.as_mut() {
            instance.serial = 0;
            instance.object.unavailable();
        } else {
            inner.instance = Some(Instance {
                object: instance.clone(),
                serial: 0,
                active: false,
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
        F: FnOnce(&mut Instance),
    {
        let mut inner = self.inner.lock().unwrap();
        if let Some(instance) = inner.instance.as_mut() {
            f(instance);
        }
    }

    /// Callback function to access the input method.
    pub(crate) fn with_input_method<F>(&self, mut f: F)
    where
        F: FnMut(&mut InputMethod),
    {
        let mut inner = self.inner.lock().unwrap();
        f(&mut inner);
    }

    pub(crate) fn set_cursor_rectangle<D: SeatHandler + 'static>(
        &self,
        state: &mut D,
        cursor: Rectangle<i32, Logical>,
    ) {
        let mut inner = self.inner.lock().unwrap();
        inner.cursor_rectangle = cursor;

        let (geometry, repositioned) = if let Some(instance) = &inner.instance {
            let instance = instance.object.data::<InputMethodUserData<D>>().unwrap();
            (
                instance.popup_geometry_callback.clone(),
                instance.popup_repositioned.clone(),
            )
        } else {
            // When is there no instance? If the input method is gone, then the popup should be done too, so this seems redundant.
            warn!("No instance of the input method???");
            return;
        };
        for popup_surface in &mut inner.popup_handles {
            let popup_geometry = (geometry)(
                state,
                &popup_surface.get_parent().surface,
                &cursor,
                &popup_surface.positioner,
            );

            let anchor = cursor; // FIXME: choose the anchor which the positioner wants

            popup_surface.set_position(ImPopupLocation {
                anchor,
                geometry: popup_geometry,
            });

            // TODO: send now or on .done?
            (repositioned)(state, popup_surface.clone());
        }
    }

    pub(crate) fn done(&self) {
        let mut inner = self.inner.lock().unwrap();

        for popup_surface in &mut inner.popup_handles {
            popup_surface.send_pending_configure();
        }
        inner.instance.as_mut().map(|i| i.done());
    }

    /// Activate input method on the given surface.
    pub(crate) fn activate_input_method<D: SeatHandler + 'static>(&self, _: &mut D, _surface: &WlSurface) {
        self.with_input_method(|im| {
            if let Some(instance) = im.instance.as_mut() {
                instance.object.activate();
                instance.active = true;
            }
        });
    }

    /// Deactivate the active input method.
    ///
    /// This includes a complete sequence including .done.
    pub(crate) fn deactivate_input_method<D: SeatHandler + 'static>(&self, state: &mut D) {
        self.with_input_method(|im| {
            if let Some(instance) = im.instance.as_mut() {
                instance.object.deactivate();
                instance.done();
                instance.active = false;
                for popup in im.popup_handles.drain(..) {
                    let data = instance.object.data::<InputMethodUserData<D>>().unwrap();
                    (data.dismiss_popup)(state, popup.clone());
                }
            }
        });
    }
}

/// User data of XxInputMethodV1 object
#[derive(Clone)]
pub struct InputMethodUserData<D: SeatHandler> {
    pub(super) handle: InputMethodHandle,
    pub(crate) text_input_handle: TextInputHandle,
    pub(crate) parent_geometry: fn(&D, &WlSurface) -> Rectangle<i32, Logical>,
    /// Returns the position of the popup, given the cursor rectangle expressed in position relative to surface
    pub(crate) popup_geometry_callback: fn(
        &D,
        // parent surface
        &WlSurface,
        // cursor position
        &Rectangle<i32, Logical>,
        &PositionerState,
    ) -> Rectangle<i32, Logical>,
    pub(crate) new_popup: fn(&mut D, PopupSurface),
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
                if let Some(instance) = &input_method.instance {
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
                                    "Popup may only be created on an active input method (no surface in tet input focus).",
                                );
                                return;
                            }
                        };

                        let location = state.parent_geometry(&parent_surface);
                        let parent = PopupParent {
                            surface: parent_surface,
                            location,
                        };
                        let configure_tracker = Arc::new(Mutex::new(Default::default()));
                        let popup_state = Arc::new(Mutex::new(PopupSurfaceState::new_uninit()));

                        let instance = data_init.init(
                            id,
                            InputMethodPopupSurfaceUserData {
                                alive_tracker: AliveTracker::default(),
                                surface: surface.clone(),
                                configure_tracker: configure_tracker.clone(),
                                state: popup_state.clone(),
                            },
                        );

                        let positioner_data = *positioner
                            .data::<PositionerUserData>()
                            .unwrap()
                            .inner
                            .lock()
                            .unwrap();

                        let geometry = (data.popup_geometry_callback)(
                            &state,
                            &parent.surface,
                            &input_method.cursor_rectangle,
                            &positioner_data,
                        );
                        // TODO: feed the popup with the anchor chosen by the positioner
                        let popup = PopupSurface::new(
                            im.clone(),
                            instance,
                            surface,
                            parent,
                            input_method.cursor_rectangle,
                            geometry,
                            positioner_data,
                            configure_tracker,
                            popup_state,
                        );
                        input_method.popup_handles.push(popup.clone());
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
