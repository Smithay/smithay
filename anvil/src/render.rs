use smithay::{
    backend::renderer::{
        output::{
            element::{
                surface::WaylandSurfaceRenderElement, texture::TextureRenderElement, RenderElement, Wrap,
            },
            OutputRender, OutputRenderError,
        },
        ImportAll, Renderer,
    },
    desktop::{
        self,
        space::{Space, SpaceElement},
    },
    utils::{Physical, Rectangle},
};

use crate::{drawing::CLEAR_COLOR, shell::FullscreenSurface};

smithay::backend::renderer::output::element::render_elements! {
    OutputRenderElements<'a, R, E>;
    Space=smithay::backend::renderer::output::element::Wrap<E>,
    Custom=&'a E,
}

pub fn render_output<R, C, E>(
    output_render: &mut OutputRender,
    space: &Space,
    space_elements: &[C],
    output_elements: &[E],
    renderer: &mut R,
    age: usize,
    log: &slog::Logger,
) -> Result<Option<Vec<Rectangle<i32, Physical>>>, OutputRenderError<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
    C: SpaceElement<R, E>,
    E: RenderElement<R>
        + From<WaylandSurfaceRenderElement>
        + From<TextureRenderElement<<R as Renderer>::TextureId>>,
{
    let output = output_render.output();

    if let Some(window) = output
        .user_data()
        .get::<FullscreenSurface>()
        .and_then(|f| f.get())
    {
        let scale = output.current_scale().fractional_scale().into();
        let window_render_elements = SpaceElement::<R, E>::render_elements(&window, (0, 0).into(), scale);

        let output_geo = space.output_geometry(output).unwrap();

        let output_render_elements = output_elements
            .iter()
            .map(|e| OutputRenderElements::Custom(e))
            .chain(
                space_elements
                    .iter()
                    .filter(|e| {
                        let geometry = e.geometry(space.id());
                        output_geo.overlaps(geometry)
                    })
                    .flat_map(|e| {
                        let location = e.location(space.id()) - output_geo.loc;
                        e.render_elements(location.to_physical_precise_round(scale), scale)
                    })
                    .chain(window_render_elements)
                    .map(|e| OutputRenderElements::Space(Wrap::from(e))),
            )
            .collect::<Vec<_>>();

        output_render.render_output(renderer, age, &*output_render_elements, CLEAR_COLOR, log)
    } else {
        desktop::space::render_output(
            renderer,
            age,
            &[(space, space_elements)],
            output_elements,
            output_render,
            CLEAR_COLOR,
            log,
        )
    }
}
