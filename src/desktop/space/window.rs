use crate::{
    backend::renderer::{Frame, ImportAll, Renderer, Texture},
    desktop::{
        space::{Space, SpaceElement},
        window::{draw_window, Window},
    },
    utils::{Logical, Point, Rectangle},
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

pub fn window_geo(window: &Window, space_id: &usize) -> Rectangle<i32, Logical> {
    let loc = window_loc(window, space_id);
    let mut wgeo = window.geometry();
    wgeo.loc = loc;
    wgeo
}

pub fn window_rect(window: &Window, space_id: &usize) -> Rectangle<i32, Logical> {
    let loc = window_loc(window, space_id);
    let mut wgeo = window.bbox();
    wgeo.loc += loc;
    wgeo
}

pub fn window_rect_with_popups(window: &Window, space_id: &usize) -> Rectangle<i32, Logical> {
    let loc = window_loc(window, space_id);
    let mut wgeo = window.bbox_with_popups();
    wgeo.loc += loc;
    wgeo
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

impl<R, F, E, T> SpaceElement<R, F, E, T> for Window
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error,
    T: Texture + 'static,
{
    fn id(&self) -> usize {
        self.0.id
    }

    fn type_of(&self) -> TypeId {
        TypeId::of::<Window>()
    }

    fn location(&self, space_id: usize) -> Point<i32, Logical> {
        window_loc(self, &space_id)
    }

    fn geometry(&self, space_id: usize) -> Rectangle<i32, Logical> {
        window_rect_with_popups(self, &space_id)
    }

    fn accumulated_damage(&self, for_values: Option<(&Space, &Output)>) -> Vec<Rectangle<i32, Logical>> {
        self.accumulated_damage(for_values)
    }

    fn draw(
        &self,
        space_id: usize,
        renderer: &mut R,
        frame: &mut F,
        scale: f64,
        location: Point<i32, Logical>,
        damage: &[Rectangle<i32, Logical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error> {
        let res = draw_window(renderer, frame, self, scale, location, damage, log);
        if res.is_ok() {
            window_state(space_id, self).drawn = true;
        }
        res
    }

    fn z_index(&self) -> u8 {
        RenderZindex::Shell as u8
    }
}
