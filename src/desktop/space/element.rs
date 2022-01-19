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

/// Enum for indicating on with layer a render element schould be draw
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RenderLayer {
    /// Bellow every other elements
    Bottom,
    /// Above WlrLayer::Background but bellow WlrLayer::Bottom
    AboveBackground,
    /// Right before programs windows are draw
    BeforeWindows,
    /// Right after programs windows are draw
    AfterWindows,
    /// Above WlrLayer::Top but bellow WlrLayer::Overlay
    BeforeOverlay,
    /// Above anything else
    Top,
}

/// Elements rendered by [`Space::render_output`] in addition to windows, layers and popups.
pub type DynamicRenderElements<R> =
    Box<dyn RenderElement<R, <R as Renderer>::Frame, <R as Renderer>::Error, <R as Renderer>::TextureId>>;

pub(super) type SpaceElem<R> =
    dyn SpaceElement<R, <R as Renderer>::Frame, <R as Renderer>::Error, <R as Renderer>::TextureId>;

/// Helper struct for iterating over diffrent layers of `DynamicRenderElements`
pub(super) struct DynamicRenderElementMap<'a, R: Renderer>(pub(super) &'a [DynamicRenderElements<R>]);

impl<'a, R> DynamicRenderElementMap<'a, R>
where
    R: Renderer + ImportAll + 'static,
    R::TextureId: 'static,
    R::Error: 'static,
    R::Frame: 'static,
{
    /// Iterate over `DynamicRenderElements` with layer `RenderLayer::Bottom`
    pub fn iter_bottom(&'a self) -> Box<dyn Iterator<Item = &SpaceElem<R>> + 'a> {
        self.iter_layer(RenderLayer::Bottom)
    }

    /// Iterate over `DynamicRenderElements with layer `RenderLayer::AboveBackground`
    pub fn iter_above_background(&'a self) -> Box<dyn Iterator<Item = &SpaceElem<R>> + 'a> {
        self.iter_layer(RenderLayer::AboveBackground)
    }

    /// Iterate over `DynamicRenderElements` with layer `RenderLayer::BeforeWindows`
    pub fn iter_before_windows(&'a self) -> Box<dyn Iterator<Item = &SpaceElem<R>> + 'a> {
        self.iter_layer(RenderLayer::BeforeWindows)
    }

    /// Iterate over `DynamicRenderElements` with layer `RenderLayer::AfterWindows`
    pub fn iter_after_windows(&'a self) -> Box<dyn Iterator<Item = &SpaceElem<R>> + 'a> {
        self.iter_layer(RenderLayer::AfterWindows)
    }

    /// Iterate over `DynamicRenderElements` with layer `RenderLayer::BeforeOverlay`
    pub fn iter_before_overlay(&'a self) -> Box<dyn Iterator<Item = &SpaceElem<R>> + 'a> {
        self.iter_layer(RenderLayer::BeforeOverlay)
    }

    /// Iterate over `DynamicRenderElements` with layer `RenderLayer::Top`
    pub fn iter_top(&'a self) -> Box<dyn Iterator<Item = &SpaceElem<R>> + 'a> {
        self.iter_layer(RenderLayer::Top)
    }

    /// Iterate over `DynamicRenderElements` with provided `layer`
    pub fn iter_layer(&'a self, layer: RenderLayer) -> Box<dyn Iterator<Item = &SpaceElem<R>> + 'a> {
        Box::new(
            self.0
                .iter()
                .filter(move |c| c.layer() == layer)
                .map(|c| c as &SpaceElem<R>),
        )
    }
}

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

    /// Returns they layer the elements schould be draw on, defaults to Top
    fn layer(&self) -> RenderLayer {
        RenderLayer::Top
    }
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
