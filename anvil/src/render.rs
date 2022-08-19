use smithay::{
    backend::renderer::{
        damage::{DamageTrackedRenderer, DamageTrackedRendererError, DamageTrackedRendererMode},
        element::{
            surface::WaylandSurfaceRenderElement, texture::TextureRenderElement, AsRenderElements,
            RenderElement,
        },
        ImportAll, Renderer,
    },
    desktop::{
        self,
        space::{Space, SpaceElement},
        Window,
    },
    output::Output,
    render_elements,
    utils::{Physical, Rectangle},
    wayland::output::Output,
};

use crate::{
    drawing::{PointerRenderElement, CLEAR_COLOR},
    shell::FullscreenSurface,
};

smithay::backend::renderer::element::render_elements! {
    pub CustomRenderElements<R>;
    Pointer=PointerRenderElement<R>,
    Surface=WaylandSurfaceRenderElement,
    #[cfg(feature = "debug")]
    Fps=FpsElement<<R as Renderer>::TextureId>,
}

#[allow(clippy::too_many_arguments)]
pub fn render_output<'a, R>(
    output: &Output,
    space: &'a Space<Window>,
    custom_elements: &'a [CustomRenderElements<R>],
    renderer: &mut R,
    damage_tracked_renderer: &mut DamageTrackedRenderer,
    age: usize,
    log: &slog::Logger,
) -> Result<Option<Vec<Rectangle<i32, Physical>>>, DamageTrackedRendererError<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    if let Some(window) = output
        .user_data()
        .get::<FullscreenSurface>()
        .and_then(|f| f.get())
    {
        if let DamageTrackedRendererMode::Auto(renderer_output) = damage_tracked_renderer.mode() {
            assert!(renderer_output == output);
        }

        let scale = output.current_scale().fractional_scale().into();
        let window_render_elements = AsRenderElements::<R>::render_elements(&window, (0, 0).into(), scale);

        let output_geo = space.output_geometry(output).unwrap();

        let render_elements = custom_elements
            .iter()
            .chain(window_render_elements.iter())
            .collect::<Vec<_>>();

        damage_tracked_renderer.render_output(renderer, age, &render_elements, CLEAR_COLOR, log)
    } else {
        desktop::space::render_output(
            output,
            renderer,
            age,
            &[space],
            custom_elements,
            damage_tracked_renderer,
            CLEAR_COLOR,
            log,
        )
    }
}
