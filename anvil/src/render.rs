use smithay::{
    backend::{
        color::CMS,
        renderer::{
            damage::{Error as OutputDamageTrackerError, OutputDamageTracker},
            element::{
                surface::WaylandSurfaceRenderElement,
                utils::{
                    ConstrainAlign, ConstrainScaleBehavior, CropRenderElement, RelocateRenderElement,
                    RescaleRenderElement,
                },
                AsRenderElements, RenderElement, RenderElementStates, Wrap,
            },
            ImportAll, ImportMem, Renderer,
        },
    },
    desktop::space::{
        constrain_space_element, ConstrainBehavior, ConstrainReference, Space, SpaceRenderElements,
    },
    output::Output,
    utils::{Physical, Point, Rectangle, Size},
};

#[cfg(feature = "debug")]
use crate::drawing::FpsElement;
use crate::{
    drawing::{PointerRenderElement, CLEAR_COLOR, CLEAR_COLOR_FULLSCREEN},
    shell::{FullscreenSurface, WindowElement, WindowRenderElement},
};

smithay::backend::renderer::element::render_elements! {
    pub CustomRenderElements<R> where
        R: ImportAll + ImportMem;
    Pointer=PointerRenderElement<R, CMS>,
    Surface=WaylandSurfaceRenderElement<R>,
    #[cfg(feature = "debug")]
    // Note: We would like to borrow this element instead, but that would introduce
    // a feature-dependent lifetime, which introduces a lot more feature bounds
    // as the whole type changes and we can't have an unused lifetime (for when "debug" is disabled)
    // in the declaration.
    Fps=FpsElement<<R as Renderer>::TextureId>,
}

impl<R: Renderer + std::fmt::Debug, C: CMS + std::fmt::Debug> std::fmt::Debug for CustomRenderElements<R, C>
where
    <R as Renderer>::TextureId: std::fmt::Debug,
    <C as CMS>::ColorProfile: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pointer(arg0) => f.debug_tuple("Pointer").field(arg0).finish(),
            Self::Surface(arg0) => f.debug_tuple("Surface").field(arg0).finish(),
            #[cfg(feature = "debug")]
            Self::Fps(arg0) => f.debug_tuple("Fps").field(arg0).finish(),
            Self::_GenericCatcher(arg0) => f.debug_tuple("_GenericCatcher").field(arg0).finish(),
        }
    }
}

type PreviewElement<R, CMS> =
    CropRenderElement<RelocateRenderElement<RescaleRenderElement<WindowRenderElement<R, CMS>>>>;
smithay::backend::renderer::element::render_elements! {
    pub OutputRenderElements<R, E> where R: ImportAll + ImportMem;
    Space=SpaceRenderElements<R, CMS, E>,
    Window=Wrap<E>,
    Custom=CustomRenderElements<R, CMS>,
    Preview=PreviewElement<R, CMS>,
}

impl<
        R: Renderer + ImportAll + ImportMem + std::fmt::Debug,
        C: CMS + std::fmt::Debug,
        E: RenderElement<R, C> + std::fmt::Debug,
    > std::fmt::Debug for OutputRenderElements<R, C, E>
where
    <R as Renderer>::TextureId: std::fmt::Debug,
    <C as CMS>::ColorProfile: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Space(arg0) => f.debug_tuple("Space").field(arg0).finish(),
            Self::Window(arg0) => f.debug_tuple("Window").field(arg0).finish(),
            Self::Custom(arg0) => f.debug_tuple("Custom").field(arg0).finish(),
            Self::Preview(arg0) => f.debug_tuple("Preview").field(arg0).finish(),
            Self::_GenericCatcher(arg0) => f.debug_tuple("_GenericCatcher").field(arg0).finish(),
        }
    }
}

impl<R, C, E> From<CustomRenderElements<R, C>> for OutputRenderElements<R, C, E>
where
    R: Renderer + ImportAll + ImportMem,
    C: CMS,
    E: RenderElement<R, C>,
{
    fn from(elem: CustomRenderElements<R, C>) -> Self {
        OutputRenderElements::Custom(elem)
    }
}

impl<R, C, E> From<SpaceRenderElements<R, C, E>> for OutputRenderElements<R, C, E>
where
    R: Renderer + ImportAll + ImportMem,
    C: CMS,
    E: RenderElement<R, C>,
{
    fn from(elem: SpaceRenderElements<R, C, E>) -> Self {
        OutputRenderElements::Space(elem)
    }
}

impl<R, C, E> From<PreviewElement<R, C>> for OutputRenderElements<R, C, E>
where
    R: Renderer + ImportAll + ImportMem,
    C: CMS,
    E: RenderElement<R, C>,
{
    fn from(elem: PreviewElement<R, C>) -> Self {
        OutputRenderElements::Preview(elem)
    }
}

pub fn space_preview_elements<'a, R, C, E>(
    renderer: &'a mut R,
    cms: &'a mut C,
    space: &'a Space<WindowElement>,
    output: &'a Output,
) -> impl Iterator<Item = E> + 'a
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + 'static,
    C: CMS + 'static,
    E: From<CropRenderElement<RelocateRenderElement<RescaleRenderElement<WindowRenderElement<R, C>>>>> + 'a,
{
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

    space
        .elements_for_output(output)
        .enumerate()
        .flat_map(move |(element_index, window)| {
            let column = element_index % elements_per_row;
            let row = element_index / elements_per_row;
            let preview_location = Point::from((
                preview_padding + (preview_padding + preview_size.w) * column as i32,
                preview_padding + (preview_padding + preview_size.h) * row as i32,
            ));
            let constrain = Rectangle::from_loc_and_size(preview_location, preview_size);
            constrain_space_element(
                renderer,
                cms,
                window,
                preview_location,
                output_scale,
                constrain,
                constrain_behavior,
            )
        })
}

pub fn output_elements<'a, R, C>(
    output: &Output,
    space: &'a Space<WindowElement>,
    custom_elements: impl IntoIterator<Item = CustomRenderElements<R, C>>,
    renderer: &mut R,
    cms: &mut C,
    show_window_preview: bool,
) -> (
    Vec<OutputRenderElements<R, C, WindowRenderElement<R, C>>>,
    [f32; 4],
)
where
    C: CMS + 'static,
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + 'static,
{
    if let Some(window) = output
        .user_data()
        .get::<FullscreenSurface>()
        .and_then(|f| f.get())
    {
        let scale = output.current_scale().fractional_scale().into();
        let window_render_elements: Vec<WindowRenderElement<R, C>> =
            AsRenderElements::<R, C>::render_elements(&window, renderer, cms, (0, 0).into(), scale);

        let elements = custom_elements
            .into_iter()
            .map(OutputRenderElements::from)
            .chain(
                window_render_elements
                    .into_iter()
                    .map(|e| OutputRenderElements::Window(Wrap::from(e))),
            )
            .collect::<Vec<_>>();
        (elements, CLEAR_COLOR_FULLSCREEN)
    } else {
        let mut output_render_elements = custom_elements
            .into_iter()
            .map(OutputRenderElements::from)
            .collect::<Vec<_>>();

        if show_window_preview && space.elements_for_output(output).count() > 0 {
            output_render_elements.extend(space_preview_elements(renderer, cms, space, output));
        }

        let space_elements = smithay::desktop::space::space_render_elements::<_, _, WindowElement, _>(
            renderer,
            cms,
            [space],
            output,
        )
        .expect("output without mode?");
        output_render_elements.extend(space_elements.into_iter().map(OutputRenderElements::Space));

        (output_render_elements, CLEAR_COLOR)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_output<'a, R, C: CMS + 'static>(
    output: &Output,
    space: &'a Space<WindowElement>,
    custom_elements: impl IntoIterator<Item = CustomRenderElements<R, C>>,
    renderer: &mut R,
    cms: &mut C,
    output_profile: &<C as CMS>::ColorProfile,
    damage_tracker: &mut OutputDamageTracker,
    age: usize,
    show_window_preview: bool,
) -> Result<(Option<Vec<Rectangle<i32, Physical>>>, RenderElementStates), OutputDamageTrackerError<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + 'static,
{
    let (elements, clear_color) =
        output_elements(output, space, custom_elements, renderer, cms, show_window_preview);
    let srgb_profile = cms.profile_srgb();
    damage_tracker.render_output(
        renderer,
        cms,
        age,
        &elements,
        clear_color,
        &srgb_profile,
        output_profile,
    )
}
