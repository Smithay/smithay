use smithay::{
    backend::renderer::{
        damage::{DamageTrackedRenderer, DamageTrackedRendererError, DamageTrackedRendererMode},
        element::{surface::WaylandSurfaceRenderElement, AsRenderElements},
        ImportAll, ImportMem, Renderer,
    },
    desktop::{self, space::Space},
    utils::{Physical, Rectangle},
    wayland::output::Output,
};

#[cfg(feature = "debug")]
use crate::drawing::FpsElement;
use crate::{
    drawing::{PointerRenderElement, CLEAR_COLOR},
    shell::FullscreenSurface,
    ssd::DecoratedWindow,
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
    space: &'a Space<DecoratedWindow>,
    custom_elements: &'a [CustomRenderElements<R>],
    renderer: &mut R,
    damage_tracked_renderer: &mut DamageTrackedRenderer,
    age: usize,
    log: &slog::Logger,
) -> Result<Option<Vec<Rectangle<i32, Physical>>>, DamageTrackedRendererError<R>>
where
    R: Renderer + ImportAll + ImportMem,
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

        let render_elements = custom_elements
            .iter()
            .chain(window_render_elements.iter())
            .collect::<Vec<_>>();

        damage_tracked_renderer.render_output(renderer, age, &render_elements, CLEAR_COLOR, log)
    } else {
        desktop::space::render_output::<R, CustomRenderElements<R>, DecoratedWindow>(
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
