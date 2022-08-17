use crate::{
    backend::renderer::{Frame, Renderer, Texture},
    utils::{Physical, Point, Rectangle, Scale, Transform},
};

use super::{Id, RenderElement, UnderlyingStorage};

/// A single texture render element
#[derive(Debug)]
pub struct TextureRenderElement<T: Texture> {
    location: Point<i32, Physical>,
    id: Id,
    texture: T,
}

impl<T: Texture> TextureRenderElement<T> {
    /// Create a texture render element from an existing texture
    pub fn from_texture(location: impl Into<Point<i32, Physical>>, id: Id, texture: T) -> Self {
        Self {
            location: location.into(),
            id,
            texture,
        }
    }
}

impl<R, T> RenderElement<R> for TextureRenderElement<T>
where
    T: Texture,
    R: Renderer<TextureId = T>,
{
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> usize {
        1
    }

    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        self.location
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::from_loc_and_size(self.location, (64, 64))
    }

    fn damage_since(&self, scale: Scale<f64>, commit: Option<usize>) -> Vec<Rectangle<i32, Physical>> {
        vec![]
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        vec![]
    }

    fn underlying_storage(&self, renderer: &R) -> Option<UnderlyingStorage<'_, R>> {
        todo!()
    }

    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error> {
        frame.render_texture_at(
            &self.texture,
            self.location,
            1,
            scale,
            Transform::Normal,
            damage,
            1.0,
        )
    }
}
