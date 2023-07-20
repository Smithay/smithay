use wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_popup_surface_v2::{
    self, ZwpInputPopupSurfaceV2,
};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::{protocol::wl_surface::WlSurface, Dispatch, Resource};

use crate::utils::{
    alive_tracker::{AliveTracker, IsAlive},
    Physical, Rectangle,
};

use super::InputMethodManagerState;

/// A handle to an input method popup surface
#[derive(Debug, Clone)]
pub struct PopupSurface {
    pub surface_role: ZwpInputPopupSurfaceV2,
    surface: WlSurface,
    parent: WlSurface,
    pub rectangle: Rectangle<i32, Physical>,
}

impl std::cmp::PartialEq for PopupSurface {
    fn eq(&self, other: &Self) -> bool {
        self.surface_role == other.surface_role
    }
}

impl PopupSurface {
    pub(crate) fn new(surface_role: ZwpInputPopupSurfaceV2, surface: WlSurface, parent: WlSurface) -> Self {
        Self {
            surface_role,
            surface,
            parent,
            rectangle: Default::default(),
        }
    }

    pub fn alive(&self) -> bool {
        // TODO other things to check? This may not sufice.
        let role_data: &InputMethodPopupSurfaceUserData = self.surface_role.data().unwrap();
        self.surface.alive() && role_data.alive_tracker.alive()
    }

    pub fn wl_surface(&self) -> &WlSurface {
        &self.surface
    }

    pub fn get_parent_surface(&self) -> WlSurface {
        self.parent.clone()
    }

    /// Used to access the location of an input popup surface relative to the parent
    pub fn rectangle(&self) -> Rectangle<i32, Physical> {
        self.rectangle
    }

    /// Set relative location of text cursor
    pub fn set_rectangle(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.rectangle = Rectangle::from_loc_and_size((x, y), (width, height));
        self.surface_role.text_input_rectangle(x, y, width, height);
    }
}

/// User data of ZwpInputPopupSurfaceV2 object
#[derive(Debug)]
pub struct InputMethodPopupSurfaceUserData {
    pub(super) alive_tracker: AliveTracker,
}

impl<D> Dispatch<ZwpInputPopupSurfaceV2, InputMethodPopupSurfaceUserData, D> for InputMethodManagerState {
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &ZwpInputPopupSurfaceV2,
        request: zwp_input_popup_surface_v2::Request,
        _data: &InputMethodPopupSurfaceUserData,
        _dhandle: &wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_input_popup_surface_v2::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(_state: &mut D, _client: ClientId, _id: ObjectId, data: &InputMethodPopupSurfaceUserData) {
        data.alive_tracker.destroy_notify();
    }
}
