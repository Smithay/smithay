use crate::{
    backend::renderer::{
        element::{
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            RenderElement,
        },
        ImportAll, Renderer, Texture,
    },
    desktop::{window::Window, PopupManager},
    utils::{Logical, Physical, Point, Rectangle, Scale},
};
use std::{
    cell::{RefCell, RefMut},
    collections::HashMap,
};

use super::SpaceElement;

#[derive(Default)]
pub struct WindowState {
    pub location: Point<i32, Logical>,
    pub drawn: bool,
    pub z_index: u8,
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

impl<R, E> SpaceElement<R, E> for Window
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Texture + 'static,
    E: RenderElement<R> + From<WaylandSurfaceRenderElement>,
{
    fn location(&self, space_id: usize) -> Point<i32, Logical> {
        window_loc(self, &space_id) - self.geometry().loc
    }

    fn geometry(&self, space_id: usize) -> Rectangle<i32, Logical> {
        window_rect(self, &space_id)
    }

    fn z_index(&self, space_id: usize) -> u8 {
        window_state(space_id, self).z_index
    }

    fn render_elements(&self, location: Point<i32, Physical>, scale: Scale<f64>) -> Vec<E> {
        let surface = self.toplevel().wl_surface();

        let mut render_elements: Vec<E> = Vec::new();
        let popup_render_elements =
            PopupManager::popups_for_surface(surface).flat_map(|(popup, popup_offset)| {
                let offset = (self.geometry().loc + popup_offset - popup.geometry().loc)
                    .to_f64()
                    .to_physical(scale)
                    .to_i32_round();

                render_elements_from_surface_tree(popup.wl_surface(), location + offset, scale)
            });

        render_elements.extend(popup_render_elements);

        render_elements.extend(render_elements_from_surface_tree(surface, location, scale));

        render_elements
    }
}
