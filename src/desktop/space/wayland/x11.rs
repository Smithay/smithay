use crate::{
    backend::renderer::{
        ImportAll, Renderer,
        element::{
            Element, Kind, NamespacedElement,
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::CropRenderElement,
        },
    },
    desktop::{WindowSurfaceType, space::SpaceElement},
    utils::{Logical, Physical, Point, Rectangle, Scale},
    xwayland::X11Surface,
};

use super::{WindowOutputUserData, output_update};

fn physical_shape_clip(
    shape: Rectangle<i32, Logical>,
    location: Point<i32, Physical>,
    scale: Scale<f64>,
) -> Rectangle<i32, Physical> {
    let mut clip = shape.to_f64().to_physical(scale).to_i32_up();
    clip.loc += location;
    clip
}

impl SpaceElement for X11Surface {
    fn bbox(&self) -> Rectangle<i32, Logical> {
        X11Surface::bbox(self)
    }

    fn geometry(&self) -> Rectangle<i32, Logical> {
        X11Surface::geometry(self)
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
    type RenderElement = NamespacedElement<CropRenderElement<WaylandSurfaceRenderElement<R>>>;

    #[profiling::function]
    fn render_elements<C: From<Self::RenderElement>>(
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
        let elements = render_elements_from_surface_tree::<R, WaylandSurfaceRenderElement<R>>(
            renderer,
            &surface,
            location,
            scale,
            alpha,
            Kind::Unspecified,
        );
        if let Some(shape) = self.render_shape() {
            let mut cropped = Vec::new();
            for (shape_index, shape_rect) in shape.iter().copied().enumerate() {
                let clip = physical_shape_clip(shape_rect, location, scale);
                cropped.extend(
                    elements
                        .iter()
                        .cloned()
                        .filter_map(|element| CropRenderElement::from_element(element, scale, clip))
                        .map(|element| NamespacedElement::new(element, shape_index))
                        .map(C::from),
                );
            }
            cropped
        } else {
            elements
                .into_iter()
                .filter_map(|element| {
                    let geometry = element.geometry(scale);
                    CropRenderElement::from_element(element, scale, geometry)
                })
                .map(|element| NamespacedElement::new(element, 0))
                .map(C::from)
                .collect()
        }
    }
}
