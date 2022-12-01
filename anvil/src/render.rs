use smithay::{
    backend::renderer::{
        damage::{DamageTrackedRenderer, DamageTrackedRendererError, DamageTrackedRendererMode},
        element::{
            surface::WaylandSurfaceRenderElement,
            utils::{
                ConstrainAlign, ConstrainScaleBehavior, CropRenderElement, RelocateRenderElement,
                RescaleRenderElement,
            },
            AsRenderElements, RenderElementStates,
        },
        ImportAll, Renderer,
    },
    desktop::{
        self,
        space::{constrain_space_element, ConstrainBehavior, ConstrainReference, Space},
        Window,
    },
    output::Output,
    utils::{Physical, Point, Rectangle, Size},
};

#[cfg(feature = "debug")]
use crate::drawing::FpsElement;
use crate::{
    drawing::{PointerRenderElement, CLEAR_COLOR},
    shell::FullscreenSurface,
};

smithay::backend::renderer::element::render_elements! {
    pub CustomRenderElements<R> where
        R: ImportAll;
    Pointer=PointerRenderElement<R>,
    Surface=WaylandSurfaceRenderElement<R>,
    #[cfg(feature = "debug")]
    // Note: We would like to borrow this element instead, but that would introduce
    // a feature-dependent lifetime, which introduces a lot more feature bounds
    // as the whole type changes and we can't have an unused lifetime (for when "debug" is disabled)
    // in the declaration.
    Fps=FpsElement<<R as Renderer>::TextureId>,
}

smithay::backend::renderer::element::render_elements! {
    pub OutputRenderElements<'a, R> where
        R: ImportAll;
    Custom=&'a CustomRenderElements<R>,
    Preview=CropRenderElement<RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<R>>>>,
}

#[allow(clippy::too_many_arguments)]
pub fn render_output<'a, R>(
    output: &Output,
    space: &'a Space<Window>,
    custom_elements: &'a [CustomRenderElements<R>],
    renderer: &mut R,
    damage_tracked_renderer: &mut DamageTrackedRenderer,
    age: usize,
    show_window_preview: bool,
    log: &slog::Logger,
) -> Result<(Option<Vec<Rectangle<i32, Physical>>>, RenderElementStates), DamageTrackedRendererError<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    let output_scale = output.current_scale().fractional_scale().into();

    if let Some(window) = output
        .user_data()
        .get::<FullscreenSurface>()
        .and_then(|f| f.get())
    {
        if let DamageTrackedRendererMode::Auto(renderer_output) = damage_tracked_renderer.mode() {
            assert!(renderer_output == output);
        }

        let window_render_elements =
            AsRenderElements::<R>::render_elements(&window, renderer, (0, 0).into(), output_scale);

        let render_elements = custom_elements
            .iter()
            .chain(window_render_elements.iter())
            .collect::<Vec<_>>();

        damage_tracked_renderer.render_output(renderer, age, &render_elements, CLEAR_COLOR, log.clone())
    } else {
        let mut output_render_elements = custom_elements
            .iter()
            .map(OutputRenderElements::from)
            .collect::<Vec<_>>();

        if show_window_preview && space.elements_for_output(output).count() > 0 {
            let constrain_behavior = ConstrainBehavior {
                reference: ConstrainReference::BoundingBox,
                behavior: ConstrainScaleBehavior::Fit,
                align: ConstrainAlign::CENTER,
            };

            let preview_padding = 10;

            let elements_on_space = space.elements_for_output(output).count();
            let output_scale = output.current_scale().fractional_scale();
            let output_transform = output.current_transform();
            let output_size = output
                .current_mode()
                .map(|mode| {
                    output_transform
                        .transform_size(mode.size)
                        .to_f64()
                        .to_logical(output_scale)
                })
                .unwrap_or_default();

            let max_elements_per_row = 4;
            let elements_per_row = usize::min(elements_on_space, max_elements_per_row);
            let rows = f64::ceil(elements_on_space as f64 / elements_per_row as f64);

            let preview_size = Size::from((
                f64::round(output_size.w / elements_per_row as f64) as i32 - preview_padding * 2,
                f64::round(output_size.h / rows) as i32 - preview_padding * 2,
            ));

            output_render_elements.extend(space.elements_for_output(output).enumerate().flat_map(
                |(element_index, window)| {
                    let column = element_index % elements_per_row;
                    let row = element_index / elements_per_row;
                    let preview_location = Point::from((
                        preview_padding + (preview_padding + preview_size.w) * column as i32,
                        preview_padding + (preview_padding + preview_size.h) * row as i32,
                    ));
                    let constrain = Rectangle::from_loc_and_size(preview_location, preview_size);
                    constrain_space_element(
                        renderer,
                        window,
                        preview_location,
                        output_scale,
                        constrain,
                        constrain_behavior,
                    )
                },
            ));
        }

        desktop::space::render_output(
            output,
            renderer,
            age,
            [space],
            &output_render_elements,
            damage_tracked_renderer,
            CLEAR_COLOR,
            log.clone(),
        )
    }
}
