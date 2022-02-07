use smithay::{
    backend::renderer::{Frame, ImportAll, Renderer},
    desktop::{
        draw_window,
        space::{DynamicRenderElements, RenderError, Space},
    },
    utils::{Logical, Rectangle},
    wayland::output::Output,
};

use crate::{drawing::*, shell::FullscreenSurface};

pub fn render_output<R>(
    output: &Output,
    space: &mut Space,
    renderer: &mut R,
    age: usize,
    elements: &[DynamicRenderElements<R>],
    log: &slog::Logger,
) -> Result<Option<Vec<Rectangle<i32, Logical>>>, RenderError<R>>
where
    R: Renderer + ImportAll + 'static,
    R::Frame: 'static,
    R::TextureId: 'static,
    R::Error: 'static,
{
    if let Some(window) = output
        .user_data()
        .get::<FullscreenSurface>()
        .and_then(|f| f.get())
    {
        let transform = output.current_transform().into();
        let mode = output.current_mode().unwrap();
        let scale = space.output_scale(output).unwrap();
        let output_geo = space
            .output_geometry(output)
            .unwrap_or_else(|| Rectangle::from_loc_and_size((0, 0), (0, 0)));
        renderer
            .render(mode.size, transform, |renderer, frame| {
                let mut damage = window.accumulated_damage(None);
                frame.clear(CLEAR_COLOR, &[Rectangle::from_loc_and_size((0, 0), mode.size)])?;
                draw_window(
                    renderer,
                    frame,
                    &window,
                    scale,
                    (0, 0),
                    &[Rectangle::from_loc_and_size(
                        (0, 0),
                        mode.size.to_f64().to_logical(scale).to_i32_round(),
                    )],
                    log,
                )?;
                for elem in elements {
                    let geo = elem.geometry();
                    let location = geo.loc - output_geo.loc;
                    let elem_damage = elem.accumulated_damage(None);
                    elem.draw(
                        renderer,
                        frame,
                        scale,
                        location,
                        &[Rectangle::from_loc_and_size((0, 0), geo.size)],
                        log,
                    )?;
                    damage.extend(elem_damage.into_iter().map(|mut rect| {
                        rect.loc += geo.loc;
                        rect
                    }))
                }
                Ok(Some(damage))
            })
            .and_then(std::convert::identity)
            .map_err(RenderError::<R>::Rendering)
    } else {
        space.render_output(&mut *renderer, output, age as usize, CLEAR_COLOR, &*elements)
    }
}
