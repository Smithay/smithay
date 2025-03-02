use crate::{
    backend::renderer::{sync::SyncPoint, Color32F, Frame},
    utils::{Buffer, Physical, Rectangle, Transform},
};

#[cfg(feature = "renderer_gl")]
use crate::backend::renderer::gles::GlesFrame;

#[cfg(feature = "renderer_pixman")]
use crate::backend::renderer::pixman::PixmanFrame;

use super::{AutoRendererError, AutoRendererTexture};

/// Frame for the auto renderer
#[derive(Debug)]
pub enum AutoRendererFrame<'frame, 'buffer> {
    /// Gles frame
    #[cfg(feature = "renderer_gl")]
    Gles(GlesFrame<'frame, 'buffer>),
    /// Pixman frame
    #[cfg(feature = "renderer_pixman")]
    Pixman(PixmanFrame<'frame, 'buffer>),
}

#[cfg(feature = "renderer_gl")]
impl<'frame, 'buffer> From<GlesFrame<'frame, 'buffer>> for AutoRendererFrame<'frame, 'buffer> {
    #[inline]
    fn from(value: GlesFrame<'frame, 'buffer>) -> Self {
        Self::Gles(value)
    }
}

#[cfg(feature = "renderer_pixman")]
impl<'frame, 'buffer> From<PixmanFrame<'frame, 'buffer>> for AutoRendererFrame<'frame, 'buffer> {
    #[inline]
    fn from(value: PixmanFrame<'frame, 'buffer>) -> Self {
        Self::Pixman(value)
    }
}

impl Frame for AutoRendererFrame<'_, '_> {
    type Error = AutoRendererError;

    type TextureId = AutoRendererTexture;

    fn id(&self) -> usize {
        todo!()
    }

    fn clear(&mut self, color: Color32F, at: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererFrame::Gles(frame) => frame.clear(color, at).map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererFrame::Pixman(frame) => frame.clear(color, at).map_err(AutoRendererError::from),
        }
    }

    fn draw_solid(
        &mut self,
        dest: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: Color32F,
    ) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererFrame::Gles(frame) => frame
                .draw_solid(dest, damage, color)
                .map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererFrame::Pixman(frame) => frame
                .draw_solid(dest, damage, color)
                .map_err(AutoRendererError::from),
        }
    }

    fn render_texture_at(
        &mut self,
        texture: &Self::TextureId,
        pos: crate::utils::Point<i32, Physical>,
        texture_scale: i32,
        output_scale: impl Into<crate::utils::Scale<f64>>,
        src_transform: Transform,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        alpha: f32,
    ) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererFrame::Gles(frame) => Frame::render_texture_at(
                frame,
                texture.try_into()?,
                pos,
                texture_scale,
                output_scale,
                src_transform,
                damage,
                opaque_regions,
                alpha,
            )
            .map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererFrame::Pixman(frame) => Frame::render_texture_at(
                frame,
                texture.try_into()?,
                pos,
                texture_scale,
                output_scale,
                src_transform,
                damage,
                opaque_regions,
                alpha,
            )
            .map_err(AutoRendererError::from),
        }
    }

    fn render_texture_from_to(
        &mut self,
        texture: &Self::TextureId,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        src_transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererFrame::Gles(frame) => Frame::render_texture_from_to(
                frame,
                texture.try_into()?,
                src,
                dst,
                damage,
                opaque_regions,
                src_transform,
                alpha,
            )
            .map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererFrame::Pixman(frame) => Frame::render_texture_from_to(
                frame,
                texture.try_into()?,
                src,
                dst,
                damage,
                opaque_regions,
                src_transform,
                alpha,
            )
            .map_err(AutoRendererError::from),
        }
    }

    fn transformation(&self) -> Transform {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererFrame::Gles(frame) => frame.transformation(),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererFrame::Pixman(frame) => frame.transformation(),
        }
    }

    fn wait(&mut self, sync: &SyncPoint) -> Result<(), Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererFrame::Gles(frame) => frame.wait(sync).map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererFrame::Pixman(frame) => frame.wait(sync).map_err(AutoRendererError::from),
        }
    }

    fn finish(self) -> Result<SyncPoint, Self::Error> {
        match self {
            #[cfg(feature = "renderer_gl")]
            AutoRendererFrame::Gles(frame) => frame.finish().map_err(AutoRendererError::from),
            #[cfg(feature = "renderer_pixman")]
            AutoRendererFrame::Pixman(frame) => frame.finish().map_err(AutoRendererError::from),
        }
    }
}
