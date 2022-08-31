//! TODO: Docs

use crate::{
    backend::renderer::{Frame, Renderer, Texture},
    utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform},
};

use super::{CommitCounter, Id, RenderElement, UnderlyingStorage};

/// A single texture render element
#[derive(Debug)]
pub struct TextureRenderElement<T: Texture> {
    location: Point<i32, Physical>,
    id: Id,
    texture: T,
    src: Option<Rectangle<f64, Buffer>>,
    size: Size<i32, Physical>,
    transform: Transform,
    commit: CommitCounter,
}

impl<T: Texture> TextureRenderElement<T> {
    /// Create a texture render element from an existing texture
    pub fn from_texture(
        location: impl Into<Point<i32, Physical>>,
        id: Id,
        texture: T,
        src: Option<Rectangle<f64, Buffer>>,
        size: Size<i32, Physical>,
        transform: Transform,
        commit: CommitCounter,
    ) -> Self {
        Self {
            location: location.into(),
            id,
            texture,
            src,
            size,
            transform,
            commit,
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

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::from_loc_and_size(self.location, self.size)
    }

    fn underlying_storage(&self, _renderer: &R) -> Option<UnderlyingStorage<'_, R>> {
        Some(UnderlyingStorage::External(&self.texture))
    }

    fn draw(
        &self,
        _renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        _scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        _log: &slog::Logger,
    ) -> Result<(), R::Error> {
        frame.render_texture_from_to(
            &self.texture,
            self.src
                .unwrap_or_else(|| Rectangle::from_loc_and_size((0, 0), self.texture.size()).to_f64()),
            Rectangle::from_loc_and_size(self.location, self.size),
            damage,
            self.transform,
            1.0,
        )
    }
}
