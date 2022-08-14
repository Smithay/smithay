use std::sync::{Arc, Mutex};

use wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_popup_surface_v2::{
    self, ZwpInputPopupSurfaceV2,
};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::{protocol::wl_surface::WlSurface, Dispatch};

use crate::utils::{Logical, Physical, Point, Rectangle};

use super::InputMethodManagerState;

#[derive(Default, Debug)]
pub(crate) struct InputMethodPopupSurface {
    pub surface_role: Option<ZwpInputPopupSurfaceV2>,
    pub surface: Option<WlSurface>,
    rectangle: Rectangle<i32, Physical>,
    point: Point<i32, Logical>,
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
        inner.rectangle.loc.x = x;
        inner.rectangle.loc.y = y;
        inner.rectangle.size.w = width;
        inner.rectangle.size.h = height;
    }

    /// Used to access the relative location of an input popup surface
    pub fn coordinates(&self) -> Rectangle<i32, Physical> {
        let inner = self.inner.lock().unwrap();
        let mut rectangle = inner.rectangle;
        rectangle.loc.x += inner.point.x;
        rectangle.loc.y += inner.point.y;
        rectangle
    }

    /// Sets the point of the upper left corner of the surface in focus
    pub fn set_point(&mut self, point: &Point<i32, Logical>) {
        let mut inner = self.inner.lock().unwrap();
        inner.point = *point;
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
