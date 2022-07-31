use std::sync::{Arc, Mutex};

use wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_popup_surface_v2::{
    self, ZwpInputPopupSurfaceV2,
};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::{protocol::wl_surface::WlSurface, Dispatch};

use crate::utils::{Logical, Point};

use super::InputMethodManagerState;

#[derive(Default, Debug)]
pub(crate) struct InputMethodPopupSurface {
    pub surface_role: Option<ZwpInputPopupSurfaceV2>,
    pub surface: Option<WlSurface>,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    l_x: i32,
    l_y: i32,
}

/// Handle to an input method instance
#[derive(Default, Debug, Clone)]
pub struct InputMethodPopupSurfaceHandle {
    pub(crate) inner: Arc<Mutex<InputMethodPopupSurface>>,
}

impl InputMethodPopupSurfaceHandle {
    /// Used to store surface coordinates
    pub fn add_coordinates(&self, x: i32, y: i32, width: i32, height: i32) {
        let mut inner = self.inner.lock().unwrap();
        inner.x = x;
        inner.y = y;
        inner.width = width;
        inner.height = height;
    }

    /// Used to access the relative location of an input popup surface
    pub fn coordinates(&self) -> (i32, i32, i32, i32) {
        let inner = self.inner.lock().unwrap();
        (
            inner.x + inner.l_x,
            inner.y + inner.l_y,
            inner.width,
            inner.height,
        )
    }

    /// Sets the point of the upper left corner of the surface in focus
    pub fn set_point(&mut self, point: &Point<i32, Logical>) {
        let mut inner = self.inner.lock().unwrap();
        inner.l_x = point.x;
        inner.l_y = point.y;
    }
}

/// User data of ZwpInputPopupSurfaceV2 object
#[derive(Debug)]
pub struct InputMethodPopupSurfaceUserData {
    pub(super) handle: InputMethodPopupSurfaceHandle,
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
        data.handle.inner.lock().unwrap().surface_role = None;
    }
}
