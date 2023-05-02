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

impl<R, C> AsRenderElements<R, C> for SurfaceTree
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
        crate::backend::renderer::element::surface::render_elements_from_surface_tree(
            renderer,
            cms,
            &self.surface,
            location,
            scale,
        )
    }
}
