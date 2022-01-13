#![allow(clippy::too_many_arguments, unused_imports)]

use std::{cell::RefCell, sync::Mutex};

#[cfg(feature = "image")]
use image::{ImageBuffer, Rgba};
use slog::Logger;
#[cfg(feature = "image")]
use smithay::backend::renderer::gles2::{Gles2Error, Gles2Renderer, Gles2Texture};
use smithay::{
    backend::{
        renderer::{
            buffer_type, utils::draw_surface_tree, BufferType, Frame, ImportAll, Renderer, Texture, Transform,
        },
        SwapBuffersError,
    },
    desktop::space::{RenderElement, Space, SurfaceTree, SpaceOutputTuple},
    reexports::wayland_server::protocol::{wl_buffer, wl_surface},
    utils::{Logical, Point, Rectangle, Size},
    wayland::{
        compositor::{
            get_role, with_states, with_surface_tree_upward, Damage, SubsurfaceCachedState,
            SurfaceAttributes, TraversalAction,
        },
        output::Output,
        seat::CursorImageAttributes,
        shell::wlr_layer::Layer,
    },
};

use crate::shell::SurfaceData;

pub static CLEAR_COLOR: [f32; 4] = [0.8, 0.8, 0.9, 1.0];

pub fn draw_cursor<R, F, E, T>(
    surface: wl_surface::WlSurface,
    location: impl Into<Point<i32, Logical>>,
    log: &Logger,
) -> impl RenderElement<R, F, E, T>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll + 'static,
    F: Frame<Error = E, TextureId = T> + 'static,
    E: std::error::Error + Into<SwapBuffersError> + 'static,
    T: Texture + 'static,
{
    let mut position = location.into();
    let ret = with_states(&surface, |states| {
        Some(
            states
                .data_map
                .get::<Mutex<CursorImageAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .hotspot,
        )
    })
    .unwrap_or(None);
    position -= match ret {
        Some(h) => h,
        None => {
            warn!(
                log,
                "Trying to display as a cursor a surface that does not have the CursorImage role."
            );
            (0, 0).into()
        }
    };
    SurfaceTree { surface, position }
}

pub fn draw_dnd_icon<R, F, E, T>(
    surface: wl_surface::WlSurface,
    location: impl Into<Point<i32, Logical>>,
    log: &Logger,
) -> impl RenderElement<R, F, E, T>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll + 'static,
    F: Frame<Error = E, TextureId = T> + 'static,
    E: std::error::Error + Into<SwapBuffersError> + 'static,
    T: Texture + 'static,
{
    if get_role(&surface) != Some("dnd_icon") {
        warn!(
            log,
            "Trying to display as a dnd icon a surface that does not have the DndIcon role."
        );
    }
    SurfaceTree {
        surface,
        position: location.into(),
    }
}

pub struct PointerElement<T: Texture> {
    texture: T,
    position: Point<i32, Logical>,
    size: Size<i32, Logical>,
}

impl<T: Texture> PointerElement<T> {
    pub fn new(texture: T, relative_pointer_pos: Point<i32, Logical>) -> PointerElement<T> {
        let size = texture.size().to_logical(1);
        PointerElement {
            texture,
            position: relative_pointer_pos,
            size,
        }
    }
}

impl<R, F, E, T> RenderElement<R, F, E, T> for PointerElement<T>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll + 'static,
    F: Frame<Error = E, TextureId = T> + 'static,
    E: std::error::Error + Into<SwapBuffersError> + 'static,
    T: Texture + 'static,
{
    fn id(&self) -> usize {
        0
    }

    fn geometry(&self) -> Rectangle<i32, Logical> {
        Rectangle::from_loc_and_size(self.position, self.size)
    }

    fn accumulated_damage(&self, _: Option<SpaceOutputTuple<'_, '_>>) -> Vec<Rectangle<i32, Logical>> {
        vec![Rectangle::from_loc_and_size((0, 0), self.size)]
    }

    fn draw(
        &self,
        _renderer: &mut R,
        frame: &mut F,
        scale: f64,
        damage: &[Rectangle<i32, Logical>],
        _log: &Logger,
    ) -> Result<(), R::Error> {
        frame.render_texture_at(
            &self.texture,
            self.position.to_f64().to_physical(scale as f64).to_i32_round(),
            1,
            scale as f64,
            Transform::Normal,
            &*damage
                .iter()
                .map(|rect| rect.to_f64().to_physical(scale).to_i32_round())
                .collect::<Vec<_>>(),
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
impl<R, F, E, T> RenderElement<R, F, E, T> for FpsElement<T>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll + 'static,
    F: Frame<Error = E, TextureId = T> + 'static,
    E: std::error::Error + Into<SwapBuffersError> + 'static,
    T: Texture + 'static,
{
    fn id(&self) -> usize {
        0
    }

    fn geometry(&self) -> Rectangle<i32, Logical> {
        let digits = if self.value < 10 {
            1
        } else if self.value < 100 {
            2
        } else {
            3
        };
        Rectangle::from_loc_and_size((0, 0), (24 * digits, 35))
    }

    fn accumulated_damage(&self, _: Option<SpaceOutputTuple<'_, '_>>) -> Vec<Rectangle<i32, Logical>> {
        vec![Rectangle::from_loc_and_size((0, 0), (24 * 3, 35))]
    }

    fn draw(
        &self,
        _renderer: &mut R,
        frame: &mut F,
        scale: f64,
        damage: &[Rectangle<i32, Logical>],
        _log: &Logger,
    ) -> Result<(), R::Error> {
        let value_str = std::cmp::min(self.value, 999).to_string();
        let mut offset_x = 0;
        for digit in value_str.chars().map(|d| d.to_digit(10).unwrap()) {
            let damage = damage
                .iter()
                .flat_map(|x| {
                    x.intersection(Rectangle::from_loc_and_size(
                        Point::from((offset_x as i32, 0)),
                        (22, 35),
                    ))
                })
                .map(|mut x| {
                    x.loc = (0, 0).into();
                    x.to_f64().to_physical(scale).to_i32_round()
                })
                .collect::<Vec<_>>();
            frame.render_texture_from_to(
                &self.texture,
                match digit {
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
                },
                Rectangle::from_loc_and_size(
                    Point::from(((offset_x as f64) * scale, 0.0)),
                    (22.0 * scale, 35.0 * scale),
                ),
                &damage,
                Transform::Normal,
                1.0,
            )?;
            offset_x += 24;
        }

        Ok(())
    }
}

#[cfg(feature = "debug")]
pub fn draw_fps<R, F, E, T>(texture: &T, value: u32) -> impl RenderElement<R, F, E, T>
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll + 'static,
    F: Frame<Error = E, TextureId = T> + 'static,
    E: std::error::Error + Into<SwapBuffersError> + 'static,
    T: Texture + Clone + 'static,
{
    FpsElement {
        value,
        texture: texture.clone(),
    }
}

#[cfg(feature = "image")]
pub fn import_bitmap<C: std::ops::Deref<Target = [u8]>>(
    renderer: &mut Gles2Renderer,
    image: &ImageBuffer<Rgba<u8>, C>,
) -> Result<Gles2Texture, Gles2Error> {
    use smithay::backend::renderer::gles2::ffi;

    renderer.with_context(|renderer, gl| unsafe {
        let mut tex = 0;
        gl.GenTextures(1, &mut tex);
        gl.BindTexture(ffi::TEXTURE_2D, tex);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
        gl.TexImage2D(
            ffi::TEXTURE_2D,
            0,
            ffi::RGBA as i32,
            image.width() as i32,
            image.height() as i32,
            0,
            ffi::RGBA,
            ffi::UNSIGNED_BYTE as u32,
            image.as_ptr() as *const _,
        );
        gl.BindTexture(ffi::TEXTURE_2D, 0);

        Gles2Texture::from_raw(
            renderer,
            tex,
            (image.width() as i32, image.height() as i32).into(),
        )
    })
}
