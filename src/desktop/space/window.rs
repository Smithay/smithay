use wayland_server::DisplayHandle;

use crate::{
    backend::renderer::{ImportAll, Renderer},
    desktop::{
        space::Space,
        window::{draw_window, Window},
    },
    utils::{Logical, Physical, Point, Rectangle, Scale},
    wayland::output::Output,
};
use std::{
    any::TypeId,
    cell::{RefCell, RefMut},
    collections::HashMap,
};

use super::RenderZindex;

#[derive(Default)]
pub struct WindowState {
    pub location: Point<i32, Logical>,
    pub drawn: bool,
}

pub type WindowUserdata = RefCell<HashMap<usize, WindowState>>;
pub fn window_state(space: usize, w: &Window) -> RefMut<'_, WindowState> {
    let userdata = w.user_data();
    userdata.insert_if_missing(WindowUserdata::default);
    RefMut::map(userdata.get::<WindowUserdata>().unwrap().borrow_mut(), |m| {
        m.entry(space).or_default()
    })
}

pub fn window_rect(window: &Window, space_id: &usize) -> Rectangle<i32, Logical> {
    let loc = window_loc(window, space_id);
    let mut wgeo = window.bbox();
    wgeo.loc += loc;
    wgeo
}

pub fn window_physical_geometry(
    window: &Window,
    space_id: &usize,
    scale: impl Into<Scale<f64>>,
) -> Rectangle<i32, Physical> {
    let scale = scale.into();
    let loc = window_loc(window, space_id) - window.geometry().loc;
    let loc = loc.to_f64().to_physical(scale);
    window.physical_bbox_with_popups(loc, scale)
}

pub fn window_loc(window: &Window, space_id: &usize) -> Point<i32, Logical> {
    window
        .user_data()
        .get::<RefCell<HashMap<usize, WindowState>>>()
        .unwrap()
        .borrow()
        .get(space_id)
        .unwrap()
        .location
}

impl Window {
    pub(super) fn elem_id(&self) -> usize {
        self.0.id
    }

    pub(super) fn elem_type_of(&self) -> TypeId {
        TypeId::of::<Window>()
    }

    pub(super) fn elem_location(
        &self,
        space_id: usize,
        scale: impl Into<Scale<f64>>,
    ) -> Point<f64, Physical> {
        let loc = window_loc(self, &space_id) - self.geometry().loc;
        loc.to_f64().to_physical(scale)
    }

    pub(super) fn elem_geometry(
        &self,
        space_id: usize,
        scale: impl Into<Scale<f64>>,
    ) -> Rectangle<i32, Physical> {
        window_physical_geometry(self, &space_id, scale)
    }

    pub(super) fn elem_accumulated_damage(
        &self,
        space_id: usize,
        scale: impl Into<Scale<f64>>,
        for_values: Option<(&Space, &Output)>,
    ) -> Vec<Rectangle<i32, Physical>> {
        let scale = scale.into();
        let loc = window_loc(self, &space_id) - self.geometry().loc;
        self.accumulated_damage(loc.to_f64().to_physical(scale), scale, for_values)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn elem_draw<R, S>(
        &self,
        dh: &DisplayHandle,
        space_id: usize,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: S,
        location: Point<f64, Physical>,
        damage: &[Rectangle<i32, Physical>],
        log: &slog::Logger,
    ) -> Result<(), <R as Renderer>::Error>
    where
        R: Renderer + ImportAll,
        <R as Renderer>::TextureId: 'static,
        S: Into<Scale<f64>>,
    {
        let res = draw_window(dh, renderer, frame, self, scale, location, damage, log);
        if res.is_ok() {
            window_state(space_id, self).drawn = true;
        }
        res
    }

    pub(super) fn elem_z_index(&self) -> u8 {
        self.0
            .z_index
            .lock()
            .unwrap()
            .unwrap_or(RenderZindex::Shell as u8)
    }
}
