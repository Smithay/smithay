use wayland_server::protocol::wl_surface::WlSurface;

use super::*;
use crate::{
    backend::{
        color::CMS,
        renderer::{
            element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            ImportAll, Renderer,
        },
    },
    desktop::{space::SpaceElement, utils::under_from_surface_tree, WindowSurfaceType},
    utils::{Logical, Physical, Point, Rectangle, Scale},
    wayland::seat::WaylandFocus,
    xwayland::X11Surface,
};

impl WaylandFocus for X11Surface {
    fn wl_surface(&self) -> Option<WlSurface> {
        self.state.lock().unwrap().wl_surface.clone()
    }
}

impl SpaceElement for X11Surface {
    fn bbox(&self) -> Rectangle<i32, Logical> {
        let geo = X11Surface::geometry(self);
        Rectangle::from_loc_and_size((0, 0), geo.size)
    }

    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        let state = self.state.lock().unwrap();
        if let Some(surface) = state.wl_surface.as_ref() {
            under_from_surface_tree(surface, *point, (0, 0), WindowSurfaceType::ALL).is_some()
        } else {
            false
        }
    }

    fn set_activate(&self, activated: bool) {
        let _ = self.set_activated(activated);
    }

    fn output_enter(&self, output: &crate::output::Output, overlap: Rectangle<i32, Logical>) {
        self.user_data().insert_if_missing(WindowOutputUserData::default);
        {
            let mut state = self
                .user_data()
                .get::<WindowOutputUserData>()
                .unwrap()
                .borrow_mut();
            state.output_overlap.insert(output.downgrade(), overlap);
            state.output_overlap.retain(|weak, _| weak.upgrade().is_some());
        }
        self.refresh()
    }

    fn output_leave(&self, output: &crate::output::Output) {
        if let Some(state) = self.user_data().get::<WindowOutputUserData>() {
            state.borrow_mut().output_overlap.retain(|weak, _| weak != output);
        }

        let mut surface_list = output_surfaces(output);
        let state = self.state.lock().unwrap();
        let Some(surface) = state.wl_surface.as_ref() else { return; };
        with_surface_tree_downward(
            surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |wl_surface, _, _| {
                output_leave(output, &mut surface_list, wl_surface);
            },
            |_, _, _| true,
        );
    }

    fn refresh(&self) {
        self.user_data().insert_if_missing(WindowOutputUserData::default);
        let wo_state = self.user_data().get::<WindowOutputUserData>().unwrap().borrow();

        let state = self.state.lock().unwrap();
        let Some(surface) = state.wl_surface.as_ref() else { return };
        for (weak, overlap) in wo_state.output_overlap.iter() {
            if let Some(output) = weak.upgrade() {
                output_update(&output, *overlap, surface);
            }
        }
    }

    fn z_index(&self) -> u8 {
        if self.is_override_redirect() {
            crate::desktop::space::RenderZindex::Overlay as u8
        } else {
            crate::desktop::space::RenderZindex::Shell as u8
        }
    }
}

impl<R, C> crate::backend::renderer::element::AsRenderElements<R, C> for X11Surface
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
    C: CMS,
    C::ColorProfile: 'static,
{
    type RenderElement = WaylandSurfaceRenderElement<R>;

    fn render_elements<I: From<WaylandSurfaceRenderElement<R>>>(
        &self,
        renderer: &mut R,
        cms: &mut C,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
    ) -> Vec<I> {
        let state = self.state.lock().unwrap();
        let Some(surface) = state.wl_surface.as_ref() else { return Vec::new() };
        render_elements_from_surface_tree(renderer, cms, surface, location, scale)
    }
}
