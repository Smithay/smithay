use crate::{
    backend::renderer::Renderer,
    desktop::space::*,
    utils::{Physical, Point, Scale},
};
use wayland_server::protocol::wl_surface::WlSurface;

/// A custom surface tree
#[derive(Debug)]
pub struct SurfaceTree {
    surface: WlSurface,
}

impl SurfaceTree {
    /// Create a surface tree from a surface
    pub fn from_surface(surface: &WlSurface) -> Self {
        SurfaceTree {
            surface: surface.clone(),
        }
    }
}

impl IsAlive for SurfaceTree {
    fn alive(&self) -> bool {
        self.surface.alive()
    }
}

impl<R> AsRenderElements<R> for SurfaceTree
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
        crate::backend::renderer::element::surface::render_elements_from_surface_tree(
            &self.surface,
            location,
            scale,
        )
    }
}
