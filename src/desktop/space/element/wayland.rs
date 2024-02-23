use crate::{
    backend::renderer::{
        element::{surface::WaylandSurfaceRenderElement, AsRenderElements, Kind},
        ImportAll, Renderer,
    },
    utils::{IsAlive, Physical, Point, Scale},
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
    type RenderElement = WaylandSurfaceRenderElement<R>;

    #[profiling::function]
    fn render_elements<C: From<WaylandSurfaceRenderElement<R>>>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        crate::backend::renderer::element::surface::render_elements_from_surface_tree(
            renderer,
            &self.surface,
            location,
            scale,
            alpha,
            Kind::Unspecified,
        )
    }
}
