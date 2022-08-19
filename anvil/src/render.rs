use smithay::{
    backend::renderer::{
        output::{
            element::{
                surface::WaylandSurfaceRenderElement, texture::TextureRenderElement, RenderElement, Wrap,
            },
            DamageTrackedRenderer, Mode, OutputRenderError,
        },
        ImportAll, Renderer,
    },
    desktop::{
        self,
        space::{Space, SpaceElement},
    },
    utils::{Physical, Rectangle},
    wayland::output::Output,
};

use crate::{drawing::CLEAR_COLOR, shell::FullscreenSurface};

smithay::backend::renderer::output::element::render_elements! {
    OutputRenderElements<'a, R, E>;
    Space=smithay::backend::renderer::output::element::Wrap<E>,
    Custom=&'a E,
}

#[allow(clippy::too_many_arguments)]
pub fn render_output<R, C, E>(
    output: &Output,
    space: &Space,
    space_elements: &[C],
    output_elements: &[E],
    renderer: &mut R,
    damage_tracked_renderer: &mut DamageTrackedRenderer,
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
    if let Some(window) = output
        .user_data()
        .get::<FullscreenSurface>()
        .and_then(|f| f.get())
    {
        if let Mode::Auto(renderer_output) = damage_tracked_renderer.mode() {
            assert!(renderer_output == output);
        }

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

        damage_tracked_renderer.render_output(renderer, age, &*output_render_elements, CLEAR_COLOR, log)
    } else {
        desktop::space::render_output(
            output,
            renderer,
            age,
            &[(space, space_elements)],
            output_elements,
            damage_tracked_renderer,
            CLEAR_COLOR,
            log,
        )
    }
}
