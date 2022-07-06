#![allow(clippy::too_many_arguments)]

use std::sync::Mutex;

use slog::Logger;
#[cfg(feature = "debug")]
use smithay::utils::Buffer;
use smithay::{
    backend::renderer::{Frame, ImportAll, Renderer, Texture},
    desktop::space::{RenderElement, SpaceOutputTuple, SurfaceTree},
    reexports::wayland_server::protocol::wl_surface,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::{
        compositor::{get_role, with_states},
        seat::CursorImageAttributes,
    },
};

pub static CLEAR_COLOR: [f32; 4] = [0.8, 0.8, 0.9, 1.0];

smithay::custom_elements! {
    pub CustomElem<R>;
    SurfaceTree=SurfaceTree,
    PointerElement=PointerElement::<<R as Renderer>::TextureId>,
    #[cfg(feature = "debug")]
    FpsElement=FpsElement::<<R as Renderer>::TextureId>,
}

pub fn draw_cursor(
    surface: wl_surface::WlSurface,
    location: impl Into<Point<i32, Logical>>,
    _log: &Logger,
) -> SurfaceTree {
    let mut position = location.into();
    position -= with_states(&surface, |states| {
        states
            .data_map
            .get::<Mutex<CursorImageAttributes>>()
            .unwrap()
            .lock()
            .unwrap()
            .hotspot
    });
    SurfaceTree {
        surface,
        position,
        z_index: 100, /* Cursor should always be on-top */
    }
}

pub fn draw_dnd_icon(
    surface: wl_surface::WlSurface,
    location: impl Into<Point<i32, Logical>>,
    log: &Logger,
) -> SurfaceTree {
    if get_role(&surface) != Some("dnd_icon") {
        warn!(
            log,
            "Trying to display as a dnd icon a surface that does not have the DndIcon role."
        );
    }
    SurfaceTree {
        surface,
        position: location.into(),
        z_index: 100, /* Cursor should always be on-top */
    }
}

pub struct PointerElement<T: Texture> {
    texture: T,
    position: Point<i32, Logical>,
    size: Size<i32, Logical>,
}

impl<T: Texture> PointerElement<T> {
    pub fn new(texture: T, pointer_pos: Point<i32, Logical>) -> PointerElement<T> {
        let size = texture.size().to_logical(1, Transform::Normal);
        PointerElement {
            texture,
            position: pointer_pos,
            size,
        }
    }
}

impl<R> RenderElement<R> for PointerElement<<R as Renderer>::TextureId>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    fn id(&self) -> usize {
        0
    }

    fn location(&self, scale: impl Into<Scale<f64>>) -> Point<f64, Physical> {
        self.position.to_f64().to_physical(scale)
    }

    fn geometry(&self, scale: impl Into<Scale<f64>>) -> Rectangle<i32, Physical> {
        Rectangle::from_loc_and_size(self.position, self.size).to_physical_precise_round(scale)
    }

    fn accumulated_damage(
        &self,
        scale: impl Into<Scale<f64>>,
        _: Option<SpaceOutputTuple<'_, '_>>,
    ) -> Vec<Rectangle<i32, Physical>> {
        let scale = scale.into();
        vec![Rectangle::from_loc_and_size(self.position, self.size).to_physical_precise_up(scale)]
    }

    fn opaque_regions(&self, _scale: impl Into<Scale<f64>>) -> Option<Vec<Rectangle<i32, Physical>>> {
        None
    }

    fn draw(
        &self,
        _renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: impl Into<Scale<f64>>,
        location: Point<f64, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _log: &Logger,
    ) -> Result<(), <R as Renderer>::Error> {
        let scale = scale.into();
        frame.render_texture_at(
            &self.texture,
            location.to_i32_round(),
            1,
            scale,
            Transform::Normal,
            &[Rectangle::from_loc_and_size(
                (0, 0),
                self.size.to_physical_precise_round(scale),
            )],
            1.0,
        )?;
        Ok(())
    }
}

#[cfg(feature = "debug")]
pub static FPS_NUMBERS_PNG: &[u8] = include_bytes!("../resources/numbers.png");

#[cfg(feature = "debug")]
pub struct FpsElement<T: Texture> {
    value: u32,
    texture: T,
}

#[cfg(feature = "debug")]
impl<R> RenderElement<R> for FpsElement<<R as Renderer>::TextureId>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    fn id(&self) -> usize {
        0
    }

    fn location(&self, _scale: impl Into<Scale<f64>>) -> Point<f64, Physical> {
        (0.0, 0.0).into()
    }

    fn geometry(&self, scale: impl Into<Scale<f64>>) -> Rectangle<i32, Physical> {
        let digits = if self.value < 10 {
            1
        } else if self.value < 100 {
            2
        } else {
            3
        };
        Rectangle::from_loc_and_size((0, 0), (24 * digits, 35)).to_physical_precise_round(scale)
    }

    fn accumulated_damage(
        &self,
        scale: impl Into<Scale<f64>>,
        _: Option<SpaceOutputTuple<'_, '_>>,
    ) -> Vec<Rectangle<i32, Physical>> {
        vec![Rectangle::from_loc_and_size((0, 0), (24 * 3, 35)).to_physical_precise_up(scale)]
    }

    fn opaque_regions(&self, _scale: impl Into<Scale<f64>>) -> Option<Vec<Rectangle<i32, Physical>>> {
        None
    }

    fn draw(
        &self,
        _renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: impl Into<Scale<f64>>,
        location: Point<f64, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _log: &Logger,
    ) -> Result<(), <R as Renderer>::Error> {
        let value_str = std::cmp::min(self.value, 999).to_string();
        let scale = scale.into();
        let mut offset: Point<f64, Physical> = Point::from((0.0, 0.0));
        for digit in value_str.chars().map(|d| d.to_digit(10).unwrap()) {
            let digit_location = location + offset;
            let digit_size = Size::<i32, Logical>::from((22, 35)).to_f64().to_physical(scale);
            let dst = Rectangle::from_loc_and_size(
                digit_location.to_i32_round(),
                ((digit_size.to_point() + digit_location).to_i32_round() - digit_location.to_i32_round())
                    .to_size(),
            );
            let damage = damage
                .iter()
                .cloned()
                .flat_map(|x| x.intersection(dst))
                .map(|mut x| {
                    x.loc -= dst.loc;
                    x
                })
                .collect::<Vec<_>>();
            let src: Rectangle<i32, Buffer> = match digit {
                9 => Rectangle::from_loc_and_size((0, 0), (22, 35)),
                6 => Rectangle::from_loc_and_size((22, 0), (22, 35)),
                3 => Rectangle::from_loc_and_size((44, 0), (22, 35)),
                1 => Rectangle::from_loc_and_size((66, 0), (22, 35)),
                8 => Rectangle::from_loc_and_size((0, 35), (22, 35)),
                0 => Rectangle::from_loc_and_size((22, 35), (22, 35)),
                2 => Rectangle::from_loc_and_size((44, 35), (22, 35)),
                7 => Rectangle::from_loc_and_size((0, 70), (22, 35)),
                4 => Rectangle::from_loc_and_size((22, 70), (22, 35)),
                5 => Rectangle::from_loc_and_size((44, 70), (22, 35)),
                _ => unreachable!(),
            };
            frame.render_texture_from_to(
                &self.texture,
                src.to_f64(),
                dst,
                &damage,
                Transform::Normal,
                1.0,
            )?;
            offset += Point::from((24.0, 0.0)).to_physical(scale);
        }

        Ok(())
    }
}

#[cfg(feature = "debug")]
pub fn draw_fps<R>(texture: &<R as Renderer>::TextureId, value: u32) -> FpsElement<<R as Renderer>::TextureId>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Clone,
{
    FpsElement {
        value,
        texture: texture.clone(),
    }
}
