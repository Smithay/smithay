use crate::{
    backend::renderer::{
        element::{
            utils::{
                constrain_as_render_elements, ConstrainAlign, ConstrainScaleBehavior, CropRenderElement,
                RelocateRenderElement, RescaleRenderElement,
            },
            AsRenderElements,
        },
        Renderer,
    },
    utils::{Logical, Point, Rectangle, Scale},
};

use super::SpaceElement;

/// Defines the reference size for the constrain behavior.
#[derive(Debug, Copy, Clone)]
pub enum ConstrainReference {
    /// Use the bounding box as the reference
    BoundingBox,
    /// Use the geometry as the reference
    Geometry,
}

/// Defines the behavior for [`constrain_space_element`]
#[derive(Debug, Copy, Clone)]
pub struct ConstrainBehavior {
    /// Defines what should be used as the reference for calculating the scale
    pub reference: ConstrainReference,
    /// Defines how the element should be scaled
    pub behavior: ConstrainScaleBehavior,
    /// Defines the alignment of the element inside of the constrain
    pub align: ConstrainAlign,
}

/// Constrain the render elements of a [`SpaceElement`]
///
/// see [`constrain_as_render_elements`]
#[profiling::function]
pub fn constrain_space_element<'a, R, E, L, S>(
    renderer: &'a mut R,
    element: &'a E,
    location: L,
    alpha: f32,
    scale: S,
    constrain: Rectangle<i32, Logical>,
    behavior: ConstrainBehavior,
) -> impl Iterator<
    Item = CropRenderElement<
        RelocateRenderElement<RescaleRenderElement<<E as AsRenderElements<R>>::RenderElement>>,
    >,
> + use<'a, R, E, L, S>
where
    R: Renderer,
    E: SpaceElement + AsRenderElements<R>,
    L: Into<Point<i32, Logical>>,
    S: Into<Scale<f64>>,
{
    let location = location.into();
    let scale = scale.into();

    let scale_reference = match behavior.reference {
        ConstrainReference::BoundingBox => element.bbox(),
        ConstrainReference::Geometry => element.geometry(),
    };

    constrain_as_render_elements(
        element,
        renderer,
        (location - scale_reference.loc).to_physical_precise_round(scale),
        alpha,
        constrain.to_physical_precise_round(scale),
        scale_reference.to_physical_precise_round(scale),
        behavior.behavior,
        behavior.align,
        scale,
    )
}
