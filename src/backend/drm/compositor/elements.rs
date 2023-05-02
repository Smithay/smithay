use crate::{
    backend::{
        color::CMS,
        renderer::{
            element::{Element, Id, RenderElement},
            utils::CommitCounter,
            Frame, Renderer,
        },
    },
    render_elements,
    utils::{Buffer, Physical, Point, Rectangle, Scale, Transform},
};

pub const COLOR_TRANSPARENT: [f32; 4] = [0f32, 0f32, 0f32, 0f32];

render_elements! {
    pub DrmRenderElements<'a, R, E>;
    Holepunch=HolepunchRenderElement<CMS>,
    Overlay=OverlayPlaneElement<'a, E>,
    Other=&'a E,
}

impl<'a, R, C, E> From<HolepunchRenderElement<C>> for DrmRenderElements<'a, R, C, E>
where
    R: Renderer,
    C: CMS,
    E: RenderElement<R, C>,
{
    fn from(elem: HolepunchRenderElement<C>) -> DrmRenderElements<'a, R, C, E> {
        DrmRenderElements::Holepunch(elem)
    }
}

impl<'a, R, C, E> From<OverlayPlaneElement<'a, E>> for DrmRenderElements<'a, R, C, E>
where
    R: Renderer,
    C: CMS,
    E: RenderElement<R, C>,
{
    fn from(elem: OverlayPlaneElement<'a, E>) -> DrmRenderElements<'a, R, C, E> {
        DrmRenderElements::Overlay(elem)
    }
}

pub struct HolepunchRenderElement<C: CMS> {
    id: Id,
    geometry: Rectangle<i32, Physical>,
    color_profile: C::ColorProfile,
}

impl<C: CMS> HolepunchRenderElement<C> {
    pub fn from_render_element<R, E>(id: Id, element: &E, scale: impl Into<Scale<f64>>) -> Self
    where
        R: Renderer,
        E: RenderElement<R, C>,
    {
        HolepunchRenderElement {
            id,
            geometry: element.geometry(scale.into()),
            color_profile: element.color_profile(),
        }
    }
}

impl<R, C> RenderElement<R, C> for HolepunchRenderElement<C>
where
    R: Renderer,
    C: CMS,
{
    fn draw<'frame, 'color>(
        &self,
        frame: &mut <R as Renderer>::Frame<'frame, 'color, C>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        frame.clear(
            COLOR_TRANSPARENT,
            &damage
                .iter()
                .cloned()
                .map(|mut d| {
                    d.loc += dst.loc;
                    d
                })
                .collect::<Vec<_>>(),
            &self.color_profile,
        )
    }

    fn color_profile(&self) -> <C as CMS>::ColorProfile {
        self.color_profile.clone()
    }
}

impl<C: CMS> Element for HolepunchRenderElement<C> {
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

    fn opaque_regions(&self, _scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        vec![Rectangle::from_loc_and_size(Point::default(), self.geometry.size)]
    }
}

pub struct OverlayPlaneElement<'a, E> {
    id: Id,
    element: &'a E,
}

impl<'a, E> OverlayPlaneElement<'a, E> {
    pub fn from_render_element<R, C>(id: Id, element: &'a E) -> Self
    where
        R: Renderer,
        C: CMS,
        E: RenderElement<R, C>,
    {
        OverlayPlaneElement { id, element }
    }
}

impl<'a, E> Element for OverlayPlaneElement<'a, E>
where
    E: Element,
{
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.element.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.element.src()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.element.geometry(scale)
    }

    fn transform(&self) -> Transform {
        self.element.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
        self.element.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        self.element.opaque_regions(scale)
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.element.location(scale)
    }
}

impl<'a, E, R, C> RenderElement<R, C> for OverlayPlaneElement<'a, E>
where
    E: RenderElement<R, C>,
    R: Renderer,
    C: CMS,
{
    fn draw<'draw, 'color>(
        &self,
        _frame: &mut <R as Renderer>::Frame<'draw, 'color, C>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        // We do not actually draw anything here
        Ok(())
    }

    fn color_profile(&self) -> <C as CMS>::ColorProfile {
        self.element.color_profile()
    }
}
