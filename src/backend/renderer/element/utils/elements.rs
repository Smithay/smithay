//! Utilities and helpers around the `Element` trait.

use crate::{
    backend::renderer::{
        element::{AsRenderElements, Element, Id, Kind, RenderElement, UnderlyingStorage},
        utils::DamageSet,
        Renderer,
    },
    utils::{Buffer, Physical, Point, Rectangle, Scale},
};

/// A element that allows to re-scale another element
#[derive(Debug)]
pub struct RescaleRenderElement<E> {
    element: E,
    origin: Point<i32, Physical>,
    scale: Scale<f64>,
}

impl<E: Element> RescaleRenderElement<E> {
    /// Create a new re-scale element for an existing element
    ///
    /// The origin can be used to scale the element geometry relative to a [`Point`].
    /// One use case of this is to only scale the location for some elements in a group, like
    /// a surface tree.
    pub fn from_element(element: E, origin: Point<i32, Physical>, scale: impl Into<Scale<f64>>) -> Self {
        RescaleRenderElement {
            element,
            origin,
            scale: scale.into(),
        }
    }
}

impl<E: Element> Element for RescaleRenderElement<E> {
    fn id(&self) -> &Id {
        self.element.id()
    }

    fn current_commit(&self) -> crate::backend::renderer::utils::CommitCounter {
        self.element.current_commit()
    }

    fn src(&self) -> crate::utils::Rectangle<f64, crate::utils::Buffer> {
        self.element.src()
    }

    fn geometry(
        &self,
        scale: crate::utils::Scale<f64>,
    ) -> crate::utils::Rectangle<i32, crate::utils::Physical> {
        let mut element_geometry = self.element.geometry(scale);
        // First we make the element relative to the origin
        element_geometry.loc -= self.origin;
        // Then we scale it by our scale
        element_geometry = element_geometry.to_f64().upscale(self.scale).to_i32_round();
        // At last we move it back to the origin
        element_geometry.loc += self.origin;
        element_geometry
    }

    fn transform(&self) -> crate::utils::Transform {
        self.element.transform()
    }

    fn damage_since(
        &self,
        scale: crate::utils::Scale<f64>,
        commit: Option<crate::backend::renderer::utils::CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.element
            .damage_since(scale, commit)
            .into_iter()
            .map(|rect| rect.to_f64().upscale(self.scale).to_i32_up())
            .collect::<DamageSet<_, _>>()
    }

    fn opaque_regions(
        &self,
        scale: crate::utils::Scale<f64>,
    ) -> Vec<crate::utils::Rectangle<i32, crate::utils::Physical>> {
        self.element
            .opaque_regions(scale)
            .into_iter()
            .map(|rect| rect.to_f64().upscale(self.scale).to_i32_round())
            .collect::<Vec<_>>()
    }

    fn alpha(&self) -> f32 {
        self.element.alpha()
    }

    fn kind(&self) -> Kind {
        self.element.kind()
    }
}

impl<R: Renderer, E: RenderElement<R>> RenderElement<R> for RescaleRenderElement<E> {
    fn draw(
        &self,
        frame: &mut <R as Renderer>::Frame<'_>,
        src: crate::utils::Rectangle<f64, crate::utils::Buffer>,
        dst: crate::utils::Rectangle<i32, crate::utils::Physical>,
        damage: &[crate::utils::Rectangle<i32, crate::utils::Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        self.element.draw(frame, src, dst, damage)
    }

    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage> {
        self.element.underlying_storage(renderer)
    }
}

/// A element that allows to crop another element
#[derive(Debug)]
pub struct CropRenderElement<E> {
    element: E,
    src: Rectangle<f64, Buffer>,
    crop_rect: Rectangle<i32, Physical>,
}

impl<E: Element> CropRenderElement<E> {
    /// Create a cropping render element for an existing element
    ///
    /// The crop rect is expected to be relative to the same origin the element is relative to.
    /// It can extend outside of the element geometry but the resulting geometry will always
    /// be equal or smaller than the geometry of the original element.
    ///
    /// The scale is used to calculate the intersection between the crop rect and the
    /// original element geometry and should therefore equal the scale that was used to
    /// calculate the crop rect.
    ///
    /// Returns `None` if there is no overlap between the crop rect and the element geometry
    pub fn from_element(
        element: E,
        scale: impl Into<Scale<f64>>,
        crop_rect: Rectangle<i32, Physical>,
    ) -> Option<Self> {
        let scale = scale.into();

        let element_geometry = element.geometry(scale);

        if let Some(intersection) = element_geometry.intersection(crop_rect) {
            // FIXME: intersection sometimes return a 0 size element
            if intersection.is_empty() {
                return None;
            }

            // We need to know how much we should crop from the element
            let mut element_relative_intersection = intersection;
            element_relative_intersection.loc -= element_geometry.loc;

            // Now we calculate the scale from the original element geometry to the
            // original element src. We need that scale to bring our intersection
            // into buffer space.
            // We also have to consider the buffer transform for this or otherwise
            // the scale would be wrong
            let element_src = element.src();
            let transform = element.transform();
            let physical_to_buffer_scale =
                element_src.size / transform.invert().transform_size(element_geometry.size).to_f64();

            // Ok, for the src we need to know how much we cropped from the element geometry
            // and then bring that rectangle into buffer space. For this we have to first
            // apply the element transform and then scale it to buffer space.
            let mut src = element_relative_intersection.to_f64().to_logical(1.0).to_buffer(
                physical_to_buffer_scale,
                transform,
                &element_geometry.size.to_f64().to_logical(1.0),
            );

            // Ensure cropping of the existing element is respected.
            src.loc += element_src.loc;

            Some(CropRenderElement {
                element,
                src,
                crop_rect,
            })
        } else {
            None
        }
    }

    fn element_crop_rect(&self, scale: Scale<f64>) -> Option<Rectangle<i32, Physical>> {
        let element_geometry = self.element.geometry(scale);

        if let Some(mut intersection) = element_geometry.intersection(self.crop_rect) {
            // FIXME: intersection sometimes return a 0 size element
            if intersection.is_empty() {
                return None;
            }

            intersection.loc -= element_geometry.loc;
            Some(intersection)
        } else {
            None
        }
    }
}

impl<E: Element> Element for CropRenderElement<E> {
    fn id(&self) -> &Id {
        self.element.id()
    }

    fn current_commit(&self) -> crate::backend::renderer::utils::CommitCounter {
        self.element.current_commit()
    }

    fn src(&self) -> crate::utils::Rectangle<f64, crate::utils::Buffer> {
        self.src
    }

    fn geometry(&self, scale: Scale<f64>) -> crate::utils::Rectangle<i32, Physical> {
        let element_geometry = self.element.geometry(scale);
        if let Some(intersection) = element_geometry.intersection(self.crop_rect) {
            // FIXME: intersection sometimes return a 0 size element
            if intersection.is_empty() {
                return Default::default();
            }

            intersection
        } else {
            Default::default()
        }
    }

    fn transform(&self) -> crate::utils::Transform {
        self.element.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<crate::backend::renderer::utils::CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        if let Some(element_crop_rect) = self.element_crop_rect(scale) {
            self.element
                .damage_since(scale, commit)
                .into_iter()
                .flat_map(|rect| {
                    rect.intersection(element_crop_rect).map(|mut rect| {
                        rect.loc -= element_crop_rect.loc;
                        rect
                    })
                })
                .collect::<DamageSet<_, _>>()
        } else {
            Default::default()
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<crate::utils::Rectangle<i32, Physical>> {
        if let Some(element_crop_rect) = self.element_crop_rect(scale) {
            self.element
                .opaque_regions(scale)
                .into_iter()
                .flat_map(|rect| {
                    rect.intersection(element_crop_rect).map(|mut rect| {
                        rect.loc -= element_crop_rect.loc;
                        rect
                    })
                })
                .collect::<Vec<_>>()
        } else {
            Default::default()
        }
    }

    fn alpha(&self) -> f32 {
        self.element.alpha()
    }

    fn kind(&self) -> Kind {
        self.element.kind()
    }
}

impl<R: Renderer, E: RenderElement<R>> RenderElement<R> for CropRenderElement<E> {
    fn draw(
        &self,
        frame: &mut <R as Renderer>::Frame<'_>,
        src: crate::utils::Rectangle<f64, crate::utils::Buffer>,
        dst: crate::utils::Rectangle<i32, crate::utils::Physical>,
        damage: &[crate::utils::Rectangle<i32, crate::utils::Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        self.element.draw(frame, src, dst, damage)
    }

    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage> {
        self.element.underlying_storage(renderer)
    }
}

/// Defines how the location parameter should apply in [`RelocateRenderElement::from_element`]
#[derive(Debug, Copy, Clone)]
pub enum Relocate {
    /// The supplied location replaces the original location
    Absolute,
    /// The supplied location offsets the original location
    Relative,
}

/// A element that allows to offset the location of an existing element
#[derive(Debug)]
pub struct RelocateRenderElement<E> {
    element: E,
    relocate: Relocate,
    location: Point<i32, Physical>,
}

impl<E: Element> RelocateRenderElement<E> {
    /// Crate an re-locate element for an existing element
    pub fn from_element(element: E, location: impl Into<Point<i32, Physical>>, relocate: Relocate) -> Self {
        let location = location.into();

        RelocateRenderElement {
            element,
            location,
            relocate,
        }
    }
}

impl<E: Element> Element for RelocateRenderElement<E> {
    fn id(&self) -> &Id {
        self.element.id()
    }

    fn current_commit(&self) -> crate::backend::renderer::utils::CommitCounter {
        self.element.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.element.src()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        let mut geo = self.element.geometry(scale);
        match self.relocate {
            Relocate::Absolute => geo.loc = self.location,
            Relocate::Relative => geo.loc += self.location,
        }
        geo
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        match self.relocate {
            Relocate::Absolute => self.location,
            Relocate::Relative => self.element.location(scale) + self.location,
        }
    }

    fn transform(&self) -> crate::utils::Transform {
        self.element.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<crate::backend::renderer::utils::CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.element.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        self.element.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.element.alpha()
    }

    fn kind(&self) -> Kind {
        self.element.kind()
    }
}

impl<R: Renderer, E: RenderElement<R>> RenderElement<R> for RelocateRenderElement<E> {
    fn draw(
        &self,
        frame: &mut <R as Renderer>::Frame<'_>,
        src: crate::utils::Rectangle<f64, crate::utils::Buffer>,
        dst: crate::utils::Rectangle<i32, crate::utils::Physical>,
        damage: &[crate::utils::Rectangle<i32, crate::utils::Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        self.element.draw(frame, src, dst, damage)
    }

    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage> {
        self.element.underlying_storage(renderer)
    }
}

/// Defines the scale behavior for the constrain
#[derive(Debug, Copy, Clone)]
pub enum ConstrainScaleBehavior {
    /// Fit the element into the size, nothing will be cropped
    Fit,
    /// Zoom the element into the size, crops if the aspect ratio
    /// of the element does not match the aspect of the constrain
    /// size
    Zoom,
    /// Always stretch the element to the constrain size
    Stretch,
    /// Do not scale, but cut off at the constrain size
    CutOff,
}

bitflags::bitflags! {
    /// Defines how the elements should be aligned during constrain
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct ConstrainAlign: u8 {
        /// Align to the top
        const TOP = 0b000001;
        /// Align to the left
        const LEFT = 0b000010;
        /// Align to the right
        const RIGHT = 0b000100;
        /// Align to the bottom
        const BOTTOM = 0b001000;
        /// Align to the top left
        ///
        /// Equals TOP | LEFT
        const TOP_LEFT = Self::TOP.bits() | Self::LEFT.bits();
        /// Align to the top right
        ///
        /// Equals TOP | RIGHT
        const TOP_RIGHT = Self::TOP.bits() | Self::RIGHT.bits();
        /// Align to the bottom left
        ///
        /// Equals BOTTOM | LEFt
        const BOTTOM_LEFT = Self::BOTTOM.bits() | Self::LEFT.bits();
        /// Align to the bottom left
        ///
        /// Equals BOTTOM | LEFT
        const BOTTOM_RIGHT = Self::BOTTOM.bits() | Self::RIGHT.bits();
        /// Align to the center
        ///
        /// Equals TOP | LEFT | BOTTOM | RIGHT
        const CENTER = Self::TOP.bits() | Self::LEFT.bits() | Self::BOTTOM.bits() | Self::RIGHT.bits();
    }
}

/// Convenience function to constrain something that implements [`AsRenderElements`]
///
/// See [`constrain_render_elements`] for more information
#[profiling::function]
#[allow(clippy::too_many_arguments)]
pub fn constrain_as_render_elements<R, E, C>(
    element: &E,
    renderer: &mut R,
    location: impl Into<Point<i32, Physical>>,
    alpha: f32,
    constrain: Rectangle<i32, Physical>,
    reference: Rectangle<i32, Physical>,
    behavior: ConstrainScaleBehavior,
    align: ConstrainAlign,
    output_scale: impl Into<Scale<f64>>,
) -> impl Iterator<Item = C>
where
    R: Renderer,
    E: AsRenderElements<R>,
    C: From<
        CropRenderElement<
            RelocateRenderElement<RescaleRenderElement<<E as AsRenderElements<R>>::RenderElement>>,
        >,
    >,
{
    let location = location.into();
    let output_scale = output_scale.into();
    let elements: Vec<<E as AsRenderElements<R>>::RenderElement> =
        AsRenderElements::<R>::render_elements(element, renderer, location, output_scale, alpha);
    constrain_render_elements(
        elements,
        location,
        constrain,
        reference,
        behavior,
        align,
        output_scale,
    )
    .map(C::from)
}

/// Constrain render elements on a specific location with a specific size
///
/// * `origin` - Defines the origin for re-scaling
/// * `constrain` - Defines the rectangle on screen the elements should be constrain within
/// * `reference` - Defines the reference that should be used for constraining the elements,
///                 this is most commonly the bounding box or geometry of the elements
/// * `behavior` - Defines the behavior for scaling the elements reference in the constrain
/// * `align` - Defines how the scaled elements should be aligned within the constrain
/// * `scale` - The scale that was used to create the original elements
#[profiling::function]
pub fn constrain_render_elements<E>(
    elements: impl IntoIterator<Item = E>,
    origin: impl Into<Point<i32, Physical>>,
    constrain: Rectangle<i32, Physical>,
    reference: Rectangle<i32, Physical>,
    behavior: ConstrainScaleBehavior,
    align: ConstrainAlign,
    scale: impl Into<Scale<f64>>,
) -> impl Iterator<Item = CropRenderElement<RelocateRenderElement<RescaleRenderElement<E>>>>
where
    E: Element,
{
    let location = origin.into();
    let scale = scale.into();

    let element_scale = match behavior {
        ConstrainScaleBehavior::Fit => {
            let reference = reference.to_f64();
            let size = constrain.size.to_f64();
            let element_scale: Scale<f64> = size / reference.size;
            Scale::from(f64::min(element_scale.x, element_scale.y))
        }
        ConstrainScaleBehavior::Zoom => {
            let reference = reference.to_f64();
            let size = constrain.size.to_f64();
            let element_scale: Scale<f64> = size / reference.size;
            Scale::from(f64::max(element_scale.x, element_scale.y))
        }
        ConstrainScaleBehavior::Stretch => {
            let reference = reference.to_f64();
            let size = constrain.size.to_f64();
            size / reference.size
        }
        ConstrainScaleBehavior::CutOff => Scale::from(1.0),
    };

    let scaled_reference = reference.to_f64().upscale(element_scale);

    // Calculate the align offset
    let top_offset: f64 = if align.contains(ConstrainAlign::TOP | ConstrainAlign::BOTTOM) {
        (constrain.size.h as f64 - scaled_reference.size.h) / 2f64
    } else if align.contains(ConstrainAlign::BOTTOM) {
        constrain.size.h as f64 - scaled_reference.size.h
    } else {
        0f64
    };

    let left_offset: f64 = if align.contains(ConstrainAlign::LEFT | ConstrainAlign::RIGHT) {
        (constrain.size.w as f64 - scaled_reference.size.w) / 2f64
    } else if align.contains(ConstrainAlign::RIGHT) {
        constrain.size.w as f64 - scaled_reference.size.w
    } else {
        0f64
    };

    let align_offset: Point<f64, Physical> = Point::from((left_offset, top_offset));

    // We need to offset the elements by the reference loc or otherwise
    // the element could be positioned outside of our constrain rect.
    let reference_offset =
        reference.loc.to_f64() - Point::from((scaled_reference.loc.x, scaled_reference.loc.y));

    // Final offset
    let offset = (reference_offset + align_offset).to_i32_round();

    elements
        .into_iter()
        .map(move |e| RescaleRenderElement::from_element(e, location, element_scale))
        .map(move |e| RelocateRenderElement::from_element(e, offset, Relocate::Relative))
        .filter_map(move |e| CropRenderElement::from_element(e, scale, constrain))
}
