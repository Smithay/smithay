use std::sync::{Arc, Mutex};

use wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_popup_surface_v2::{
    self, ZwpInputPopupSurfaceV2,
};
use wayland_server::{backend::ClientId, protocol::wl_surface::WlSurface, Dispatch, Resource};

use crate::utils::{
    alive_tracker::{AliveTracker, IsAlive},
    Logical, Point, Rectangle,
};

use super::InputMethodManagerState;

/// Handle to a popup surface
#[derive(Debug, Clone, Default)]
pub struct PopupHandle {
    pub surface: Option<PopupSurface>,
    pub rectangle: Rectangle<i32, Logical>,
}

/// A handle to an input method popup surface
#[derive(Debug, Clone)]
pub struct PopupSurface {
    /// The surface role for the input method popup
    pub surface_role: ZwpInputPopupSurfaceV2,
    surface: WlSurface,
    /// Protected cursor area.
    pub(crate) rectangle: Arc<Mutex<Rectangle<i32, Logical>>>,
    /// Location of the popup surface.
    location: Arc<Mutex<Point<i32, Logical>>>,
    /// Current parent of the IME popup.
    parent: Option<PopupParent>,
}

impl PopupSurface {
    pub(crate) fn new(
        surface_role: ZwpInputPopupSurfaceV2,
        surface: WlSurface,
        rectangle: Arc<Mutex<Rectangle<i32, Logical>>>,
        parent: Option<PopupParent>,
    ) -> Self {
        let location = Arc::new(Mutex::new(rectangle.lock().unwrap().loc));
        Self {
            surface_role,
            rectangle,
            location,
            surface,
            parent,
        }
    }

    /// Is the input method popup surface referred by this handle still alive?
    #[inline]
    pub fn alive(&self) -> bool {
        // TODO other things to check? This may not sufice.
        let role_data: &InputMethodPopupSurfaceUserData = self.surface_role.data().unwrap();
        self.surface.alive() && role_data.alive_tracker.alive()
    }

    /// Access to the underlying `wl_surface` of this popup
    #[inline]
    pub fn wl_surface(&self) -> &WlSurface {
        &self.surface
    }

    /// Access to the parent surface associated with this popup
    pub fn get_parent(&self) -> Option<&PopupParent> {
        self.parent.as_ref()
    }

    /// Set the IME popup surface parent.
    pub fn set_parent(&mut self, parent: Option<PopupParent>) {
        self.parent = parent;
    }

    /// Used to access the location of an input popup surface relative to the parent
    pub fn location(&self) -> Point<i32, Logical> {
        *self.location.lock().unwrap()
    }

    /// Set location of the popup surface relative to the parent. The primary use for this function
    /// is to adjust the popup during rendering.
    ///
    /// Setting this value **won't update** the [`text_input_rectangle`].
    ///
    /// [`text_input_rectangle`]: Self::text_input_rectangle
    pub fn set_location(&self, location: Point<i32, Logical>) {
        *self.location.lock().unwrap() = location;
    }

    /// The region compositor shouldn't obscure when placing the popup within the
    /// client.
    pub fn text_input_rectangle(&self) -> Rectangle<i32, Logical> {
        *self.rectangle.lock().unwrap()
    }

    /// Set relative location of text cursor.
    ///
    /// Setting this value **will update** the [`location`] to the new `x` and `y`.
    ///
    /// [`location`]: Self::location
    pub fn set_text_input_rectangle(&mut self, x: i32, y: i32, width: i32, height: i32) {
        *self.rectangle.lock().unwrap() = Rectangle::from_loc_and_size((x, y), (width, height));
        *self.location.lock().unwrap() = (x, y).into();
        self.surface_role.text_input_rectangle(x, y, width, height);
    }
}

impl std::cmp::PartialEq for PopupSurface {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.surface_role == other.surface_role
    }
}

/// Parent surface and location for the IME popup.
#[derive(Debug, Clone)]
pub struct PopupParent {
    /// The surface IME popup is present over.
    pub surface: WlSurface,
    /// The location of the parent surface.
    pub location: Rectangle<i32, Logical>,
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

    fn destroyed(
        _state: &mut D,
        _client: ClientId,
        _object: &ZwpInputPopupSurfaceV2,
        data: &InputMethodPopupSurfaceUserData,
    ) {
        data.alive_tracker.destroy_notify();
    }
}
