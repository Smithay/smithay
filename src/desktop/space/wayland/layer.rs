use crate::{
    backend::renderer::{
        element::{
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            AsRenderElements,
        },
        ImportAll, Renderer,
    },
    desktop::{LayerSurface, PopupManager},
    utils::{Physical, Point, Scale},
};

impl<R> AsRenderElements<R> for LayerSurface
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    type RenderElement = WaylandSurfaceRenderElement<R>;

    fn render_elements<C: From<WaylandSurfaceRenderElement<R>>>(
        &self,
        renderer: &mut R,
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

                render_elements_from_surface_tree(renderer, popup.wl_surface(), location + offset, scale)
            });

        render_elements.extend(popup_render_elements);

        render_elements.extend(render_elements_from_surface_tree(
            renderer, surface, location, scale,
        ));

        render_elements
    }
}
