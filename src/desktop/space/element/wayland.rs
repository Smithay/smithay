use crate::{
    backend::renderer::Renderer,
    desktop::space::*,
    output::Output,
    utils::{Logical, Physical, Point, Rectangle, Scale},
};
use crate::{
    desktop::utils as desktop_utils,
    wayland::compositor::{with_surface_tree_downward, TraversalAction},
};

/// A custom surface tree
#[derive(Debug)]
pub struct SurfaceTree {
    location: Point<i32, Logical>,
    surface: WlSurface,
}

impl SurfaceTree {
    /// Create a surface tree from a surface
    pub fn from_surface(surface: &WlSurface, location: impl Into<Point<i32, Logical>>) -> Self {
        SurfaceTree {
            location: location.into(),
            surface: surface.clone(),
        }
    }
}

impl IsAlive for SurfaceTree {
    fn alive(&self) -> bool {
        self.surface.alive()
    }
}

impl SpaceElement for SurfaceTree {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        self.bbox()
    }

    fn bbox(&self) -> Rectangle<i32, Logical> {
        desktop_utils::bbox_from_surface_tree(&self.surface, self.location)
    }

    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        desktop_utils::under_from_surface_tree(&self.surface, *point, (0, 0), WindowSurfaceType::ALL)
            .is_some()
    }

    fn set_activate(&self, _activated: bool) {}
    fn output_enter(&self, output: &Output) {
        with_surface_tree_downward(
            &self.surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |wl_surface, _, _| {
                output.enter(wl_surface);
            },
            |_, _, _| true,
        );
    }
    fn output_leave(&self, output: &Output) {
        with_surface_tree_downward(
            &self.surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |wl_surface, _, _| {
                output.leave(wl_surface);
            },
            |_, _, _| true,
        );
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
