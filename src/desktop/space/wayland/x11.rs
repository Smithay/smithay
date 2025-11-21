use crate::{
    backend::renderer::{
        element::{
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            Kind,
        },
        ImportAll, Renderer,
    },
    desktop::{space::SpaceElement, WindowSurfaceType},
    utils::{Logical, Physical, Point, Rectangle, Scale},
    xwayland::X11Surface,
};

use super::{output_update, WindowOutputUserData};

impl SpaceElement for X11Surface {
    fn bbox(&self) -> Rectangle<i32, Logical> {
        let geo = X11Surface::geometry(self);
        Rectangle::from_size(geo.size)
    }

    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        X11Surface::surface_under(self, *point, (0, 0), WindowSurfaceType::all()).is_some()
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
            state.output_overlap.retain(|weak, _| weak.is_alive());
        }
        self.refresh()
    }

    fn output_leave(&self, output: &crate::output::Output) {
        if let Some(state) = self.user_data().get::<WindowOutputUserData>() {
            state.borrow_mut().output_overlap.retain(|weak, _| weak != output);
        }

        let Some(surface) = X11Surface::wl_surface(self) else {
            return;
        };
        output_update(output, None, &surface);
    }

    fn refresh(&self) {
        self.user_data().insert_if_missing(WindowOutputUserData::default);
        let wo_state = self.user_data().get::<WindowOutputUserData>().unwrap().borrow();

        let Some(surface) = X11Surface::wl_surface(self) else {
            return;
        };
        for (weak, overlap) in wo_state.output_overlap.iter() {
            if let Some(output) = weak.upgrade() {
                output_update(&output, Some(*overlap), &surface);
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

impl<R> crate::backend::renderer::element::AsRenderElements<R> for X11Surface
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    type RenderElement = WaylandSurfaceRenderElement<R>;

    #[profiling::function]
    fn render_elements<C: From<WaylandSurfaceRenderElement<R>>>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        mut alpha: f32,
    ) -> Vec<C> {
        let Some(surface) = X11Surface::wl_surface(self) else {
            return Vec::new();
        };
        if let Some(opacity) = self.state.lock().unwrap().opacity {
            alpha *= (opacity as f32) / (u32::MAX as f32);
        }
        render_elements_from_surface_tree(renderer, &surface, location, scale, alpha, Kind::Unspecified)
    }
}
