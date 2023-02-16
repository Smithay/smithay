use crate::{
    backend::renderer::{
        element::{Element, Id, RenderElement},
        utils::CommitCounter,
        Frame, Renderer,
    },
    render_elements,
    utils::{Buffer, Physical, Point, Rectangle, Scale, Transform},
};

pub const COLOR_TRANSPARENT: [f32; 4] = [0f32, 0f32, 0f32, 0f32];

render_elements! {
    pub DrmRenderElements<'a, R, E>;
    Holepunch=HolepunchRenderElement,
    Overlay=OverlayPlaneElement<'a, E>,
    Other=&'a E,
}

pub struct HolepunchRenderElement {
    id: Id,
    geometry: Rectangle<i32, Physical>,
}

impl HolepunchRenderElement {
    pub fn from_render_element<R, E>(element: &E, scale: impl Into<Scale<f64>>) -> Self
    where
        R: Renderer,
        E: RenderElement<R>,
    {
        HolepunchRenderElement {
            id: element.id().clone(),
            geometry: element.geometry(scale.into()),
        }
    }
}

impl<R> RenderElement<R> for HolepunchRenderElement
where
    R: Renderer,
{
    fn draw<'frame>(
        &self,
        frame: &mut <R as Renderer>::Frame<'frame>,
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

    fn opaque_regions(&self, _scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        vec![Rectangle::from_loc_and_size(Point::default(), self.geometry.size)]
    }
}

pub struct OverlayPlaneElement<'a, E> {
    element: &'a E,
}

impl<'a, E> OverlayPlaneElement<'a, E> {
    pub fn from_render_element<R>(element: &'a E) -> Self
    where
        R: Renderer,
        E: RenderElement<R>,
    {
        OverlayPlaneElement { element }
    }
}

impl<'a, E> Element for OverlayPlaneElement<'a, E>
where
    E: Element,
{
    fn id(&self) -> &Id {
        self.element.id()
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
        _scale: Scale<f64>,
        _commit: Option<CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
        // We do not report damage
        vec![]
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        self.element.opaque_regions(scale)
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.element.location(scale)
    }
}

impl<'a, E, R> RenderElement<R> for OverlayPlaneElement<'a, E>
where
    E: Element,
    R: Renderer,
{
    fn draw<'draw>(
        &self,
        _frame: &mut <R as Renderer>::Frame<'draw>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        // We do not actually draw anything here
        Ok(())
    }
}
