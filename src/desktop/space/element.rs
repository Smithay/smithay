use crate::backend::renderer::output::element::surface::WaylandSurfaceRenderElement;
use crate::backend::renderer::output::element::texture::TextureRenderElement;
use crate::backend::renderer::output::element::RenderElement;
use crate::{
    backend::renderer::{ImportAll, Renderer},
    desktop::space::*,
    output::Output,
    utils::{Logical, Physical, Point, Rectangle, Scale},
};
use std::hash::Hash;

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
    /// Default Layer for RenderElements
    Overlay = 60,
}

impl From<RenderZindex> for u8 {
    fn from(idx: RenderZindex) -> u8 {
        idx as u8
    }
}

impl From<RenderZindex> for Option<u8> {
    fn from(idx: RenderZindex) -> Option<u8> {
        Some(idx as u8)
    }
}

/// Trait for a space element
pub trait SpaceElement<R, E>
where
    R: Renderer + ImportAll,
    E: RenderElement<R>,
{
    /// Gets the location of this element on the specified space
    fn location(&self, space_id: usize) -> Point<i32, Logical>;
    /// Gets the geometry of this element on the specified space
    fn geometry(&self, space_id: usize) -> Rectangle<i32, Logical>;
    /// Gets the z-index of this element on the specified space
    fn z_index(&self, _space_id: usize) -> u8 {
        RenderZindex::Overlay as u8
    }
    /// Gets render elements of this space element
    fn render_elements(&self, location: Point<i32, Physical>, scale: Scale<f64>) -> Vec<E>;
}

space_elements! {
    pub(crate) SpaceElements<'a, _, C>[
        WaylandSurfaceRenderElement,
        TextureRenderElement<<R as Renderer>::TextureId>,
    ];
    Layer=&'a LayerSurface,
    Window=&'a Window,
    Custom=&'a C,
}

impl<T, R, E> SpaceElement<R, E> for &T
where
    T: SpaceElement<R, E>,
    E: RenderElement<R>,
    R: Renderer + ImportAll,
{
    fn location(&self, space_id: usize) -> Point<i32, Logical> {
        (*self).location(space_id)
    }

    fn geometry(&self, space_id: usize) -> Rectangle<i32, Logical> {
        (*self).geometry(space_id)
    }

    fn z_index(&self, space_id: usize) -> u8 {
        (*self).z_index(space_id)
    }

    fn render_elements(&self, location: Point<i32, Physical>, scale: Scale<f64>) -> Vec<E> {
        (*self).render_elements(location, scale)
    }
}
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

impl<R, E> SpaceElement<R, E> for SurfaceTree
where
    R: Renderer + ImportAll,
    E: RenderElement<R> + From<WaylandSurfaceRenderElement>,
{
    fn location(&self, _space_id: usize) -> Point<i32, Logical> {
        self.location
    }

    fn geometry(&self, _space_id: usize) -> Rectangle<i32, Logical> {
        crate::desktop::utils::bbox_from_surface_tree(&self.surface, self.location)
    }

    fn render_elements(&self, location: Point<i32, Physical>, scale: Scale<f64>) -> Vec<E> {
        crate::backend::renderer::output::element::surface::surfaces_from_surface_tree(
            &self.surface,
            location,
            scale,
        )
    }
}
