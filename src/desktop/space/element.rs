use crate::desktop::space::popup::RenderPopup;
use crate::{
    backend::renderer::{ImportAll, Renderer, Texture},
    desktop::{space::*, utils::*},
    utils::{Logical, Point, Rectangle},
    wayland::output::Output,
};
use std::{
    any::{Any, TypeId},
    hash::{Hash, Hasher},
};
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{DisplayHandle, Resource};

/// Indicates default values for some zindexs inside smithay
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum RenderZindex {
    /// WlrLayer::Background default zindex
    Background = 10,
    /// WlrLayer::Bottom default zindex
    Bottom = 20,
    /// Default zindex for Windows
    Shell = 30,
    /// WlrLayer::Top default zindex
    Top = 40,
    /// Default zindex for Windows PopUps
    Popups = 50,
    /// Default Layer for RenderElements
    Overlay = 60,
    /// Default Layer for Overlay PopUp
    PopupsOverlay = 70,
}

/// Trait for custom elements to be rendered during [`Space::render_output`].
pub trait RenderElement<R>
where
    R: Renderer + ImportAll,
    Self: Any + 'static,
{
    /// Returns an id unique to this element for the type of Self.
    fn id(&self) -> usize;
    #[doc(hidden)]
    fn type_of(&self) -> TypeId {
        Any::type_id(self)
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
    /// - `location` refers to the relative position in the bound buffer the element should be drawn at,
    ///    so that it matches with the space-relative coordinates returned by [`RenderElement::geometry`].
    /// - `damage` provides the regions you need to re-draw and *may* not
    ///   be equivalent to the damage returned by `accumulated_damage`.
    ///   Redrawing other parts of the element is not valid and may cause rendering artifacts.
    fn draw(
        &self,
        dh: &mut DisplayHandle<'_>,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: f64,
        location: Point<i32, Logical>,
        damage: &[Rectangle<i32, Logical>],
        log: &slog::Logger,
    ) -> Result<(), <R as Renderer>::Error>;

    /// Returns z_index of RenderElement, reverf too [`RenderZindex`] for default values
    fn z_index(&self) -> u8 {
        RenderZindex::Overlay as u8
    }
}

pub(crate) enum SpaceElement<'a, R, E>
where
    R: Renderer + ImportAll,
    E: RenderElement<R>,
{
    Layer(&'a LayerSurface),
    Window(&'a Window),
    Popup(&'a RenderPopup),
    Custom(&'a E, std::marker::PhantomData<R>),
}

impl<'a, R, E> SpaceElement<'a, R, E>
where
    R: Renderer + ImportAll,
    E: RenderElement<R>,
    <R as Renderer>::TextureId: 'static,
{
    pub fn id(&self) -> usize {
        match self {
            SpaceElement::Layer(layer) => layer.elem_id(),
            SpaceElement::Window(window) => window.elem_id(),
            SpaceElement::Popup(popup) => popup.elem_id(),
            SpaceElement::Custom(custom, _) => custom.id(),
        }
    }
    pub fn type_of(&self) -> TypeId {
        match self {
            SpaceElement::Layer(layer) => layer.elem_type_of(),
            SpaceElement::Window(window) => window.elem_type_of(),
            SpaceElement::Popup(popup) => popup.elem_type_of(),
            SpaceElement::Custom(custom, _) => custom.type_of(),
        }
    }
    pub fn location(&self, space_id: usize) -> Point<i32, Logical> {
        match self {
            SpaceElement::Layer(layer) => layer.elem_geometry(space_id).loc,
            SpaceElement::Window(window) => window.elem_location(space_id),
            SpaceElement::Popup(popup) => popup.elem_geometry(space_id).loc,
            SpaceElement::Custom(custom, _) => custom.geometry().loc,
        }
    }
    pub fn geometry(&self, space_id: usize) -> Rectangle<i32, Logical> {
        match self {
            SpaceElement::Layer(layer) => layer.elem_geometry(space_id),
            SpaceElement::Window(window) => window.elem_geometry(space_id),
            SpaceElement::Popup(popup) => popup.elem_geometry(space_id),
            SpaceElement::Custom(custom, _) => custom.geometry(),
        }
    }
    pub fn accumulated_damage(&self, for_values: Option<(&Space, &Output)>) -> Vec<Rectangle<i32, Logical>> {
        match self {
            SpaceElement::Layer(layer) => layer.elem_accumulated_damage(for_values),
            SpaceElement::Window(window) => window.elem_accumulated_damage(for_values),
            SpaceElement::Popup(popup) => popup.elem_accumulated_damage(for_values),
            SpaceElement::Custom(custom, _) => {
                custom.accumulated_damage(for_values.map(|(s, o)| SpaceOutputTuple(s, o)))
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        dh: &mut DisplayHandle<'_>,
        space_id: usize,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: f64,
        location: Point<i32, Logical>,
        damage: &[Rectangle<i32, Logical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error> {
        match self {
            SpaceElement::Layer(layer) => {
                layer.elem_draw(dh, space_id, renderer, frame, scale, location, damage, log)
            }
            SpaceElement::Window(window) => {
                window.elem_draw(dh, space_id, renderer, frame, scale, location, damage, log)
            }
            SpaceElement::Popup(popup) => {
                popup.elem_draw(space_id, renderer, frame, scale, location, damage, log)
            }
            SpaceElement::Custom(custom, _) => custom.draw(dh, renderer, frame, scale, location, damage, log),
        }
    }
    pub fn z_index(&self) -> u8 {
        match self {
            SpaceElement::Layer(layer) => layer.elem_z_index(),
            SpaceElement::Window(window) => window.elem_z_index(),
            SpaceElement::Popup(popup) => popup.elem_z_index(),
            SpaceElement::Custom(custom, _) => custom.z_index(),
        }
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
    /// Z-Index to draw at
    pub z_index: u8,
}

impl<R> RenderElement<R> for SurfaceTree
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Texture + 'static,
{
    fn id(&self) -> usize {
        self.surface.id().protocol_id() as usize
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
        dh: &mut DisplayHandle<'_>,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: f64,
        location: Point<i32, Logical>,
        damage: &[Rectangle<i32, Logical>],
        log: &slog::Logger,
    ) -> Result<(), <R as Renderer>::Error> {
        crate::backend::renderer::utils::draw_surface_tree(
            dh,
            renderer,
            frame,
            &self.surface,
            scale,
            location,
            damage,
            log,
        )
    }

    fn z_index(&self) -> u8 {
        self.z_index
    }
}

/// Newtype for (&Space, &Output) to provide a `Hash` implementation for damage tracking
#[derive(Debug, PartialEq)]
pub struct SpaceOutputTuple<'a, 'b>(pub &'a Space, pub &'b Output);

impl<'a, 'b> Hash for SpaceOutputTuple<'a, 'b> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state);
        (std::sync::Arc::as_ptr(&self.1.data.inner) as *const () as usize).hash(state);
    }
}

impl<'a, 'b> SpaceOutputTuple<'a, 'b> {
    /// Returns an owned version that produces and equivalent hash
    pub fn owned_hash(&self) -> SpaceOutputHash {
        SpaceOutputHash(
            self.0.id,
            std::sync::Arc::as_ptr(&self.1.data.inner) as *const () as usize,
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
