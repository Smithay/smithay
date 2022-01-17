use crate::{
    backend::renderer::{Frame, ImportAll, Renderer, Texture},
    desktop::{space::*, utils::*},
    utils::{Logical, Point, Rectangle},
    wayland::output::Output,
};
use std::{
    any::{Any, TypeId},
    hash::{Hash, Hasher},
};
use wayland_server::protocol::wl_surface::WlSurface;

/// Trait for custom elements to be rendered during [`Space::render_output`].
pub trait RenderElement<R, F, E, T>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll,
    F: Frame<Error = E, TextureId = T>,
    E: std::error::Error,
    T: Texture + 'static,
    Self: Any + 'static,
{
    /// Returns an id unique to this element for the type of Self.
    fn id(&self) -> usize;
    #[doc(hidden)]
    fn type_of(&self) -> TypeId {
        std::any::Any::type_id(self)
    }
    /// Returns the bounding box of this element including its position in the space.
    fn geometry(&self) -> Rectangle<i32, Logical>;
    /// Returns the damage of the element since it's last update.
    /// It should be relative to the elements coordinates.
    ///
    /// If you receive `Some(_)` for `for_values` you may cache that you
    /// send the damage for this `Space` and `Output` combination once
    /// and return an empty vector for subsequent calls until the contents
    /// of this element actually change again for optimization reasons.
    ///
    /// Returning `vec![Rectangle::from_loc_and_size((0, 0), (i32::MAX, i32::MAX))]` is always
    /// correct, but very inefficient.
    fn accumulated_damage(
        &self,
        for_values: Option<SpaceOutputTuple<'_, '_>>,
    ) -> Vec<Rectangle<i32, Logical>>;
    /// Draws the element using the provided `Frame` and `Renderer`.
    ///
    /// - `scale` provides the current fractional scale value to render as
    /// - `damage` provides the regions you need to re-draw and *may* not
    ///   be equivalent to the damage returned by `accumulated_damage`.
    ///   Redrawing other parts of the element is not valid and may cause rendering artifacts.
    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut F,
        scale: f64,
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
        (&**self as &dyn RenderElement<R, F, E, T>).accumulated_damage(for_values.map(SpaceOutputTuple::from))
    }
    fn draw(
        &self,
        _space_id: usize,
        renderer: &mut R,
        frame: &mut F,
        scale: f64,
        _location: Point<i32, Logical>,
        damage: &[Rectangle<i32, Logical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error> {
        (&**self as &dyn RenderElement<R, F, E, T>).draw(renderer, frame, scale, damage, log)
    }
}

/// Generic helper for drawing [`WlSurface`]s and their subsurfaces
/// as custom elements via [`RenderElement`].
///
/// For example useful for cursor or drag-and-drop surfaces.
///
/// Note: This element will render nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
#[derive(Debug)]
pub struct SurfaceTree {
    /// Surface to be drawn
    pub surface: WlSurface,
    /// Position to draw add
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

    fn accumulated_damage(
        &self,
        for_values: Option<SpaceOutputTuple<'_, '_>>,
    ) -> Vec<Rectangle<i32, Logical>> {
        damage_from_surface_tree(&self.surface, (0, 0), for_values.map(|x| (x.0, x.1)))
    }

    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut F,
        scale: f64,
        damage: &[Rectangle<i32, Logical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error> {
        crate::backend::renderer::utils::draw_surface_tree(
            renderer,
            frame,
            &self.surface,
            scale,
            self.position,
            damage,
            log,
        )
    }
}

/// Newtype for (&Space, &Output) to provide a `Hash` implementation for damage tracking
#[derive(Debug, PartialEq)]
pub struct SpaceOutputTuple<'a, 'b>(pub &'a Space, pub &'b Output);

impl<'a, 'b> Hash for SpaceOutputTuple<'a, 'b> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state);
        (std::sync::Arc::as_ptr(&self.1.inner) as *const () as usize).hash(state);
    }
}

impl<'a, 'b> SpaceOutputTuple<'a, 'b> {
    /// Returns an owned version that produces and equivalent hash
    pub fn owned_hash(&self) -> SpaceOutputHash {
        SpaceOutputHash(
            self.0.id,
            std::sync::Arc::as_ptr(&self.1.inner) as *const () as usize,
        )
    }
}

impl<'a, 'b> From<(&'a Space, &'b Output)> for SpaceOutputTuple<'a, 'b> {
    fn from((space, output): (&'a Space, &'b Output)) -> SpaceOutputTuple<'a, 'b> {
        SpaceOutputTuple(space, output)
    }
}

/// Type to use as an owned hashable value equal to [`SpaceOutputTuple`]
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct SpaceOutputHash(usize, usize);
