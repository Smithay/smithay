use crate::{
    backend::renderer::{
        element::{Element, Id, RenderElement},
        utils::{CommitCounter, DamageSet, OpaqueRegions},
        Color32F, Frame, Renderer,
    },
    render_elements,
    utils::{Buffer, Physical, Point, Rectangle, Scale, Transform},
};

render_elements! {
    pub DrmRenderElements<'a, R, E>;
    Holepunch=HolepunchRenderElement,
    Overlay=OverlayPlaneElement,
    Other=&'a E,
}

pub struct HolepunchRenderElement {
    id: Id,
    geometry: Rectangle<i32, Physical>,
}

impl HolepunchRenderElement {
    pub fn from_render_element<R, E>(id: Id, element: &E, scale: impl Into<Scale<f64>>) -> Self
    where
        R: Renderer,
        E: RenderElement<R>,
    {
        HolepunchRenderElement {
            id,
            geometry: element.geometry(scale.into()),
        }
    }
}

impl<R> RenderElement<R> for HolepunchRenderElement
where
    R: Renderer,
{
    fn draw(
        &self,
        frame: &mut <R as Renderer>::Frame<'_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        frame.clear(
            Color32F::TRANSPARENT,
            &damage
                .iter()
                .cloned()
                .map(|mut d| {
                    d.loc += dst.loc;
                    d
                })
                .collect::<Vec<_>>(),
        )
    }
}

impl Element for HolepunchRenderElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        CommitCounter::default()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::default()
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::from_slice(&[Rectangle::from_loc_and_size(Point::default(), self.geometry.size)])
    }
}

pub struct OverlayPlaneElement {
    id: Id,
    geometry: Rectangle<i32, Physical>,
    opaque_regions: OpaqueRegions<i32, Physical>,
}

impl OverlayPlaneElement {
    pub fn from_render_element<R, E>(id: Id, element: &E, scale: impl Into<Scale<f64>>) -> Option<Self>
    where
        R: Renderer,
        E: RenderElement<R>,
    {
        let scale = scale.into();
        let opaque_regions = element.opaque_regions(scale);
        let geometry = element.geometry(scale);

        // We use the opaque regions to create a bounding box around them to
        // create a fake geometry that only includes the opaque regions.
        // If the element is not fully opaque this can save the renderer
        // from some damage when the element is moved.
        let mut opaque_geometry = opaque_regions
            .iter()
            .fold(Rectangle::default(), |acc, item| acc.merge(*item));

        if opaque_geometry.is_empty() {
            return None;
        }

        // Because we will move the geometry by the top-left corner of
        // the opaque geometry we calculated we have to subtract
        // that from the element local opaque regions.
        let opaque_regions = opaque_regions
            .into_iter()
            .map(|mut region| {
                region.loc -= opaque_geometry.loc;
                region
            })
            .collect();

        // Move the opaque geometry relative to the original
        // element location
        opaque_geometry.loc += geometry.loc;

        Some(OverlayPlaneElement {
            id,
            geometry: opaque_geometry,
            opaque_regions,
        })
    }
}

impl Element for OverlayPlaneElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        CommitCounter::default()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::default()
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }

    fn damage_since(&self, _scale: Scale<f64>, _commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        DamageSet::default()
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::from_slice(&self.opaque_regions)
    }
}

impl<R> RenderElement<R> for OverlayPlaneElement
where
    R: Renderer,
{
    fn draw(
        &self,
        _frame: &mut <R as Renderer>::Frame<'_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        // We do not actually draw anything here
        Ok(())
    }
}
