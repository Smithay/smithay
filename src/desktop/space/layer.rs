use crate::{
    backend::renderer::{
        element::{
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            AsRenderElements,
        },
        ImportAll, Renderer,
    },
    desktop::{LayerSurface, PopupManager, WindowSurfaceType},
    utils::{Logical, Physical, Point, Rectangle, Scale},
    wayland::{output::Output, shell::wlr_layer::Layer},
};

use super::{RenderZindex, SpaceElement};

impl SpaceElement for LayerSurface {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        self.bbox_with_popups()
    }
    fn bbox(&self) -> Rectangle<i32, Logical> {
        self.bbox_with_popups()
    }
    fn input_region(&self, point: &Point<f64, Logical>) -> Option<Point<i32, Logical>> {
        self.surface_under(*point, WindowSurfaceType::ALL)
            .map(|(l, point)| point)
    }
    /// Gets the z-index of this element on the specified space
    fn z_index(&self) -> u8 {
        let layer = self.layer();
        let z_index = match layer {
            Layer::Background => RenderZindex::Background,
            Layer::Bottom => RenderZindex::Bottom,
            Layer::Top => RenderZindex::Top,
            Layer::Overlay => RenderZindex::Overlay,
        };
        z_index as u8
    }

    fn set_activate(&self, activated: bool) {}
    fn output_enter(&self, output: &Output) {
        output.enter(self.wl_surface())
    }
    fn output_leave(&self, output: &Output) {
        output.leave(self.wl_surface())
    }
}

impl<R> AsRenderElements<R> for LayerSurface
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    type RenderElement = WaylandSurfaceRenderElement;

    fn render_elements<C: From<WaylandSurfaceRenderElement>>(
        &self,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
    ) -> Vec<C> {
        let surface = self.wl_surface();

        let mut render_elements: Vec<C> = Vec::new();
        let popup_render_elements =
            PopupManager::popups_for_surface(surface).flat_map(|(popup, popup_offset)| {
                let offset = (popup_offset - popup.geometry().loc)
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
