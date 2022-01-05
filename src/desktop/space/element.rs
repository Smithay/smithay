use crate::{
    backend::renderer::{Frame, ImportAll, Renderer, Texture},
    desktop::{space::*, utils::*},
    utils::{Logical, Point, Rectangle},
    wayland::output::Output,
};
use std::any::{Any, TypeId};
use wayland_server::protocol::wl_surface::WlSurface;

pub trait RenderElement<R, F, E, T>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error,
    T: Texture + 'static,
    Self: Any + 'static,
{
    fn id(&self) -> usize;
    #[doc(hidden)]
    fn type_of(&self) -> TypeId {
        std::any::Any::type_id(self)
    }
    fn geometry(&self) -> Rectangle<i32, Logical>;
    fn accumulated_damage(&self, for_values: Option<(&Space, &Output)>) -> Vec<Rectangle<i32, Logical>>;
    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut F,
        scale: f64,
        location: Point<i32, Logical>,
        damage: &[Rectangle<i32, Logical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error>;
}

pub(crate) trait SpaceElement<R, F, E, T>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error,
    T: Texture,
{
    fn id(&self) -> usize;
    fn type_of(&self) -> TypeId;
    fn location(&self, space_id: usize) -> Point<i32, Logical> {
        self.geometry(space_id).loc
    }
    fn geometry(&self, space_id: usize) -> Rectangle<i32, Logical>;
    fn accumulated_damage(&self, for_values: Option<(&Space, &Output)>) -> Vec<Rectangle<i32, Logical>>;
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        space_id: usize,
        renderer: &mut R,
        frame: &mut F,
        scale: f64,
        location: Point<i32, Logical>,
        damage: &[Rectangle<i32, Logical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error>;
}

impl<R, F, E, T> SpaceElement<R, F, E, T> for Box<dyn RenderElement<R, F, E, T>>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll + 'static,
    F: Frame<Error = E, TextureId = T> + 'static,
    E: std::error::Error + 'static,
    T: Texture + 'static,
{
    fn id(&self) -> usize {
        (&**self as &dyn RenderElement<R, F, E, T>).id()
    }
    fn type_of(&self) -> TypeId {
        (&**self as &dyn RenderElement<R, F, E, T>).type_of()
    }
    fn geometry(&self, _space_id: usize) -> Rectangle<i32, Logical> {
        (&**self as &dyn RenderElement<R, F, E, T>).geometry()
    }
    fn accumulated_damage(&self, for_values: Option<(&Space, &Output)>) -> Vec<Rectangle<i32, Logical>> {
        (&**self as &dyn RenderElement<R, F, E, T>).accumulated_damage(for_values)
    }
    fn draw(
        &self,
        _space_id: usize,
        renderer: &mut R,
        frame: &mut F,
        scale: f64,
        location: Point<i32, Logical>,
        damage: &[Rectangle<i32, Logical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error> {
        (&**self as &dyn RenderElement<R, F, E, T>).draw(renderer, frame, scale, location, damage, log)
    }
}

#[derive(Debug)]
pub struct SurfaceTree {
    pub surface: WlSurface,
    pub position: Point<i32, Logical>,
}

impl<R, F, E, T> RenderElement<R, F, E, T> for SurfaceTree
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error,
    T: Texture + 'static,
{
    fn id(&self) -> usize {
        self.surface.as_ref().id() as usize
    }

    fn geometry(&self) -> Rectangle<i32, Logical> {
        let mut bbox = bbox_from_surface_tree(&self.surface, (0, 0));
        bbox.loc += self.position;
        bbox
    }

    fn accumulated_damage(&self, for_values: Option<(&Space, &Output)>) -> Vec<Rectangle<i32, Logical>> {
        damage_from_surface_tree(&self.surface, (0, 0), for_values)
    }

    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut F,
        scale: f64,
        location: Point<i32, Logical>,
        damage: &[Rectangle<i32, Logical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error> {
        crate::backend::renderer::utils::draw_surface_tree(
            renderer,
            frame,
            &self.surface,
            scale,
            location,
            damage,
            log,
        )
    }
}
