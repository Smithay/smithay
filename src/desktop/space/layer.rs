use crate::{
    backend::renderer::{
        element::{
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            RenderElement,
        },
        ImportAll, Renderer, Texture,
    },
    desktop::{
        layer::{layer_state as output_layer_state, *},
        PopupManager,
    },
    utils::{Physical, Point, Rectangle, Scale},
    wayland::shell::wlr_layer::Layer,
};

use super::{RenderZindex, SpaceElement};

impl<R, E> SpaceElement<R, E> for LayerSurface
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Texture + 'static,
    E: RenderElement<R> + From<WaylandSurfaceRenderElement>,
{
    fn location(&self, _space_id: usize) -> Point<i32, crate::utils::Logical> {
        let state = output_layer_state(self);
        state.location
    }

    fn geometry(&self, _space_id: usize) -> Rectangle<i32, crate::utils::Logical> {
        let state = output_layer_state(self);
        let mut bbox = self.bbox_with_popups();
        bbox.loc += state.location;
        bbox
    }

    fn z_index(&self, _space_id: usize) -> u8 {
        let layer = self.layer();
        let z_index = match layer {
            Layer::Background => RenderZindex::Background,
            Layer::Bottom => RenderZindex::Bottom,
            Layer::Top => RenderZindex::Top,
            Layer::Overlay => RenderZindex::Overlay,
        };
        z_index as u8
    }

    fn render_elements(&self, location: Point<i32, Physical>, scale: Scale<f64>) -> Vec<E> {
        let surface = self.wl_surface();

        let mut render_elements: Vec<E> = Vec::new();
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
