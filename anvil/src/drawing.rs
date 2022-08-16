#![allow(clippy::too_many_arguments)]

use std::sync::Mutex;

#[cfg(feature = "debug")]
use smithay::{
    backend::renderer::{
        output::element::{Id, RenderElement},
        Frame,
    },
    utils::Buffer,
};
use smithay::{
    backend::renderer::{ImportAll, Renderer, Texture},
    desktop::space::SpaceElement,
    input::pointer::{CursorImageAttributes, CursorImageStatus},
    utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::compositor::with_states,
};

pub static CLEAR_COLOR: [f32; 4] = [0.8, 0.8, 0.9, 1.0];

pub struct PointerElement<T: Texture> {
    id: smithay::backend::renderer::output::element::Id,
    texture: T,
    position: Point<i32, Logical>,
    status: CursorImageStatus,
    size: Size<i32, Logical>,
}

impl<T: Texture + 'static> PointerElement<T> {
    pub fn new(texture: T, pointer_pos: impl Into<Point<i32, Logical>>) -> PointerElement<T> {
        let size = texture.size().to_logical(1, Transform::Normal);
        PointerElement {
            id: smithay::backend::renderer::output::element::Id::new(),
            texture,
            position: pointer_pos.into(),
            size,
            status: CursorImageStatus::Default,
        }
    }

    pub fn set_position(&mut self, position: impl Into<Point<i32, Logical>>) {
        self.position = position.into();
    }

    pub fn set_status(&mut self, status: CursorImageStatus) {
        self.status = status;
    }
}

impl<T: Texture + Clone + 'static, R, E> SpaceElement<R, E> for PointerElement<T>
where
    R: Renderer<TextureId = T> + ImportAll,
    E: smithay::backend::renderer::output::element::RenderElement<R>
        + From<smithay::backend::renderer::output::element::texture::TextureRenderElement<R>>
        + From<smithay::backend::renderer::output::element::surface::WaylandSurfaceRenderElement<R>>,
{
    fn location(&self, _space_id: usize) -> Point<i32, Logical> {
        if let CursorImageStatus::Surface(surface) = &self.status {
            let hotspot = with_states(surface, |states| {
                states
                    .data_map
                    .get::<Mutex<CursorImageAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .hotspot
            });

            self.position - hotspot
        } else {
            self.position
        }
    }

    fn geometry(&self, _space_id: usize) -> Rectangle<i32, Logical> {
        match &self.status {
            CursorImageStatus::Hidden => Rectangle::default(),
            CursorImageStatus::Default => Rectangle::from_loc_and_size(self.position, self.size),
            CursorImageStatus::Surface(surface) => {
                let hotspot = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<Mutex<CursorImageAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .hotspot
                });

                smithay::desktop::utils::bbox_from_surface_tree(surface, self.position - hotspot)
            }
        }
    }

    fn render_elements(&self, location: Point<i32, Physical>, scale: Scale<f64>) -> Vec<E> {
        match &self.status {
            CursorImageStatus::Hidden => vec![],
            CursorImageStatus::Default => {
                vec![
                    smithay::backend::renderer::output::element::texture::TextureRenderElement::from_texture(
                        location,
                        self.id.clone(),
                        self.texture.clone(),
                    )
                    .into(),
                ]
            }
            CursorImageStatus::Surface(surface) => {
                smithay::backend::renderer::output::element::surface::surfaces_from_surface_tree(
                    surface, location, scale,
                )
            }
        }
    }
}

#[cfg(feature = "debug")]
pub static FPS_NUMBERS_PNG: &[u8] = include_bytes!("../resources/numbers.png");

#[cfg(feature = "debug")]
pub struct FpsElement<T: Texture> {
    id: Id,
    value: u32,
    texture: T,
    commit_counter: usize,
}

#[cfg(feature = "debug")]
impl<T: Texture> FpsElement<T> {
    pub fn new(texture: T) -> Self {
        FpsElement {
            id: Id::new(),
            texture,
            value: 0,
            commit_counter: 0,
        }
    }

    pub fn update_fps(&mut self, fps: u32) {
        if self.value != fps {
            self.value = fps;
            self.commit_counter = self.commit_counter.wrapping_add(1);
        }
    }
}

#[cfg(feature = "debug")]
impl<R> RenderElement<R> for FpsElement<<R as Renderer>::TextureId>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    fn id(&self) -> &Id {
        &self.id
    }

    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        (0, 0).into()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        let digits = if self.value < 10 {
            1
        } else if self.value < 100 {
            2
        } else {
            3
        };
        Rectangle::from_loc_and_size((0, 0), (24 * digits, 35)).to_physical_precise_round(scale)
    }

    fn current_commit(&self) -> usize {
        self.commit_counter
    }

    fn draw(
        &self,
        _renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        _log: &slog::Logger,
    ) {
        let value_str = std::cmp::min(self.value, 999).to_string();
        let mut offset: Point<f64, Physical> = Point::from((0.0, 0.0));
        for digit in value_str.chars().map(|d| d.to_digit(10).unwrap()) {
            let digit_location = offset;
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
            frame
                .render_texture_from_to(&self.texture, src.to_f64(), dst, &damage, Transform::Normal, 1.0)
                .unwrap();
            offset += Point::from((24.0, 0.0)).to_physical(scale);
        }
    }
}
