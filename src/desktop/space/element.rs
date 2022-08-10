use crate::backend::renderer::output::element::RenderElement;
use crate::{
    backend::renderer::{ImportAll, Renderer, Texture},
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

pub trait SpaceElement<R, E>
where
    R: Renderer + ImportAll,
    E: crate::backend::renderer::output::element::RenderElement<R>,
{
    fn location(&self, space_id: usize) -> Point<i32, Logical>;
    fn geometry(&self, space_id: usize) -> Rectangle<i32, Logical>;
    fn z_index(&self, _space_id: usize) -> u8 {
        RenderZindex::Overlay as u8
    }
    fn render_elements(&self, location: Point<i32, Physical>, scale: Scale<f64>) -> Vec<E>;
}

pub(crate) enum SpaceElements<'a, E> {
    Layer(&'a LayerSurface),
    Window(&'a Window),
    Custom(&'a E),
}

crate::backend::renderer::output::element::render_elements! {
    pub SpaceRenderElements<R>;
    Surface=crate::backend::renderer::output::element::surface::WaylandSurfaceRenderElement<R>,
    Texture=crate::backend::renderer::output::element::texture::TextureRenderElement<R>,
}

impl<'a, R, E> SpaceElement<R, SpaceRenderElements<R>> for SpaceElements<'a, E>
where
    R: Renderer + ImportAll + 'static,
    <R as Renderer>::TextureId: Texture + 'static,
    E: SpaceElement<R, SpaceRenderElements<R>>,
{
    fn location(&self, space_id: usize) -> Point<i32, Logical> {
        match self {
            SpaceElements::Layer(layer) => {
                SpaceElement::<R, SpaceRenderElements<R>>::location(*layer, space_id)
            }
            SpaceElements::Window(window) => {
                SpaceElement::<R, SpaceRenderElements<R>>::location(*window, space_id)
            }
            SpaceElements::Custom(custom) => {
                SpaceElement::<R, SpaceRenderElements<R>>::location(*custom, space_id)
            }
        }
    }

    fn geometry(&self, space_id: usize) -> Rectangle<i32, Logical> {
        match self {
            SpaceElements::Layer(layer) => {
                SpaceElement::<R, SpaceRenderElements<R>>::geometry(*layer, space_id)
            }
            SpaceElements::Window(window) => {
                SpaceElement::<R, SpaceRenderElements<R>>::geometry(*window, space_id)
            }
            SpaceElements::Custom(custom) => {
                SpaceElement::<R, SpaceRenderElements<R>>::geometry(*custom, space_id)
            }
        }
    }

    fn z_index(&self, space_id: usize) -> u8 {
        match self {
            SpaceElements::Layer(layer) => {
                SpaceElement::<R, SpaceRenderElements<R>>::z_index(*layer, space_id)
            }
            SpaceElements::Window(window) => {
                SpaceElement::<R, SpaceRenderElements<R>>::z_index(*window, space_id)
            }
            SpaceElements::Custom(custom) => {
                SpaceElement::<R, SpaceRenderElements<R>>::z_index(*custom, space_id)
            }
        }
    }

    fn render_elements(
        &self,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
    ) -> Vec<SpaceRenderElements<R>> {
        match self {
            SpaceElements::Layer(layer) => {
                SpaceElement::<R, SpaceRenderElements<R>>::render_elements(*layer, location, scale)
            }
            SpaceElements::Window(window) => {
                SpaceElement::<R, SpaceRenderElements<R>>::render_elements(*window, location, scale)
            }
            SpaceElements::Custom(custom) => {
                SpaceElement::<R, SpaceRenderElements<R>>::render_elements(*custom, location, scale)
            }
        }
    }
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
