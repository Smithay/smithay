//! Rendering functionality and abstractions
//!
//! Collection of common traits and implementations
//! to facilitate (possible hardware-accelerated) rendering.
//!
//! Supported rendering apis:
//!
//! - Raw OpenGL ES 2

use std::collections::HashSet;
use std::error::Error;

use cgmath::{prelude::*, Matrix3, Vector2};
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_buffer, wl_shm};

use crate::backend::SwapBuffersError;
#[cfg(feature = "renderer_gl")]
pub mod gles2;
#[cfg(feature = "wayland_frontend")]
use crate::backend::egl::EGLBuffer;

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
/// Possible transformations to two-dimensional planes
pub enum Transform {
    /// Identity transformation (plane is unaltered when applied)
    Normal,
    /// Plane is rotated by 90 degrees
    _90,
    /// Plane is rotated by 180 degrees
    _180,
    /// Plane is rotated by 270 degrees
    _270,
    /// Plane is flipped vertically
    Flipped,
    /// Plane is flipped vertically and rotated by 90 degrees
    Flipped90,
    /// Plane is flipped vertically and rotated by 180 degrees
    Flipped180,
    /// Plane is flipped vertically and rotated by 270 degrees
    Flipped270,
}

impl Transform {
    /// A projection matrix to apply this transformation
    pub fn matrix(&self) -> Matrix3<f32> {
        match self {
            Transform::Normal => Matrix3::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::_90 => Matrix3::new(0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Transform::_180 => Matrix3::new(-1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::_270 => Matrix3::new(0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped => Matrix3::new(-1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped90 => Matrix3::new(0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped180 => Matrix3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0),
            Transform::Flipped270 => Matrix3::new(0.0, -1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
        }
    }

    /// Inverts any 90-degree transformation into 270-degree transformations and vise versa.
    ///
    /// Flipping is preserved and 180/Normal transformation are uneffected.
    pub fn invert(&self) -> Transform {
        match self {
            Transform::Normal => Transform::Normal,
            Transform::Flipped => Transform::Flipped,
            Transform::_90 => Transform::_270,
            Transform::_180 => Transform::_180,
            Transform::_270 => Transform::_90,
            Transform::Flipped90 => Transform::Flipped270,
            Transform::Flipped180 => Transform::Flipped180,
            Transform::Flipped270 => Transform::Flipped90,
        }
    }

    /// Transformed size after applying this transformation.
    pub fn transform_size(&self, width: u32, height: u32) -> (u32, u32) {
        if *self == Transform::_90
            || *self == Transform::_270
            || *self == Transform::Flipped90
            || *self == Transform::Flipped270
        {
            (height, width)
        } else {
            (width, height)
        }
    }
}

#[cfg(feature = "wayland-frontend")]
impl From<wayland_server::protocol::wl_output::Transform> for Transform {
    fn from(transform: wayland_server::protocol::wl_output::Transform) -> Transform {
        use wayland_server::protocol::wl_output::Transform::*;
        match transform {
            Normal => Transform::Normal,
            _90 => Transform::_90,
            _180 => Transform::_180,
            _270 => Transform::_270,
            Flipped => Transform::Flipped,
            Flipped90 => Transform::Flipped90,
            Flipped180 => Transform::Flipped180,
            Flipped270 => Transform::Flipped270,
        }
    }
}

/// Abstraction for Renderers, that can render into different targets
pub trait Bind<Target>: Unbind {
    /// Bind a given rendering target, which will contain the rendering results until `unbind` is called.
    fn bind(&mut self, target: Target) -> Result<(), <Self as Renderer>::Error>;
    /// Supported pixel formats for given targets, if applicable.
    fn supported_formats(&self) -> Option<HashSet<crate::backend::allocator::Format>> {
        None
    }
}

/// Functionality to unbind the current rendering target
pub trait Unbind: Renderer {
    /// Unbind the current rendering target.
    ///
    /// May fall back to a default target, if defined by the implementation.
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error>;
}

/// A two dimensional texture
pub trait Texture {
    /// Size of the texture plane (w x h)
    fn size(&self) -> (u32, u32) {
        (self.width(), self.height())
    }

    /// Width of the texture plane
    fn width(&self) -> u32;
    /// Height of the texture plane
    fn height(&self) -> u32;
}

/// Abstraction of commonly used rendering operations for compositors.
pub trait Renderer {
    /// Error type returned by the rendering operations of this renderer.
    type Error: Error;
    /// Texture Handle type used by this renderer.
    type TextureId: Texture;

    /// Import a given bitmap into the renderer.
    ///
    /// Returns a texture_id, which can be used with `render_texture(_at)` or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it,
    /// and needs to be freed by calling `destroy_texture` on this renderer to avoid a resource leak.
    ///
    /// This operation needs no bound or default rendering target.
    #[cfg(feature = "image")]
    fn import_bitmap<C: std::ops::Deref<Target = [u8]>>(
        &mut self,
        image: &image::ImageBuffer<image::Rgba<u8>, C>,
    ) -> Result<Self::TextureId, Self::Error>;

    /// Returns supported formats for shared memory buffers.
    ///
    /// Will always contain At least `Argb8888` and `Xrgb8888`.
    #[cfg(feature = "wayland_frontend")]
    fn shm_formats(&self) -> &[wl_shm::Format] {
        // Mandatory
        &[wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888]
    }

    /// Import a given shared memory buffer into the renderer.
    ///
    /// Returns a texture_id, which can be used with `render_texture(_at)` or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it,
    /// and needs to be freed by calling `destroy_texture` on this renderer to avoid a resource leak.
    ///
    /// This operation needs no bound or default rendering target.
    ///
    /// The implementation defines, if the id keeps being valid, if the buffer is released,
    /// to avoid relying on implementation details, keep the buffer alive, until you destroyed this texture again.
    #[cfg(feature = "wayland_frontend")]
    fn import_shm(&mut self, buffer: &wl_buffer::WlBuffer) -> Result<Self::TextureId, Self::Error>;

    /// Import a given egl-backed memory buffer into the renderer.
    ///
    /// Returns a texture_id, which can be used with `render_texture(_at)` or implementation-specific functions.
    ///
    /// If not otherwise defined by the implementation, this texture id is only valid for the renderer, that created it,
    /// and needs to be freed by calling `destroy_texture` on this renderer to avoid a resource leak.
    ///
    /// This operation needs no bound or default rendering target.
    ///
    /// The implementation defines, if the id keeps being valid, if the buffer is released,
    /// to avoid relying on implementation details, keep the buffer alive, until you destroyed this texture again.
    #[cfg(feature = "wayland_frontend")]
    fn import_egl(&mut self, buffer: &EGLBuffer) -> Result<Self::TextureId, Self::Error>;

    /// Deallocate the given texture.
    ///
    /// In case the texture type of this renderer is cloneable or copyable, those handles will also become invalid
    /// and destroy calls with one of these handles might error out as the texture is already freed.
    fn destroy_texture(&mut self, texture: Self::TextureId) -> Result<(), Self::Error>;

    /// Initialize a rendering context on the current rendering target with given dimensions and transformation.
    ///
    /// This function *may* error, if:
    /// - The given dimensions are unsuppored (too large) for this renderer
    /// - The given Transformation is not supported by the renderer (`Transform::Normal` is always supported).
    /// - There was a previous `begin`-call, which was not terminated by `finish`.
    /// - This renderer implements `Bind`, no target was bound *and* has no default target.
    /// - (Renderers not implementing `Bind` always have a default target.)
    fn begin(
        &mut self,
        width: u32,
        height: u32,
        transform: Transform,
    ) -> Result<(), <Self as Renderer>::Error>;

    /// Finish a renderering context, previously started by `begin`.
    ///
    /// After this operation is finished the current rendering target contains a sucessfully rendered image.
    /// If the image is immediently shown to the user depends on the target.
    fn finish(&mut self) -> Result<(), SwapBuffersError>;

    /// Clear the complete current target with a single given color.
    ///
    /// This operation is only valid in between a `begin` and `finish`-call.
    /// If called outside this operation may error-out, do nothing or modify future rendering results in any way.
    fn clear(&mut self, color: [f32; 4]) -> Result<(), Self::Error>;
    /// Render a texture to the current target using given projection matrix and alpha.
    ///
    /// This operation is only valid in between a `begin` and `finish`-call.
    /// If called outside this operation may error-out, do nothing or modify future rendering results in any way.
    fn render_texture(
        &mut self,
        texture: &Self::TextureId,
        matrix: Matrix3<f32>,
        alpha: f32,
    ) -> Result<(), Self::Error>;
    /// Render a texture to the current target as a flat 2d-plane at a given
    /// position, applying the given transformation with the given alpha value.
    ///
    /// This operation is only valid in between a `begin` and `finish`-call.
    /// If called outside this operation may error-out, do nothing or modify future rendering results in any way.
    fn render_texture_at(
        &mut self,
        texture: &Self::TextureId,
        pos: (i32, i32),
        transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        let mut mat = Matrix3::<f32>::identity();

        // position and scale
        let size = texture.size();
        mat = mat * Matrix3::from_translation(Vector2::new(pos.0 as f32, pos.1 as f32));
        mat = mat * Matrix3::from_nonuniform_scale(size.0 as f32, size.1 as f32);

        //apply surface transformation
        mat = mat * Matrix3::from_translation(Vector2::new(0.5, 0.5));
        if transform == Transform::Normal {
            assert_eq!(mat, mat * transform.invert().matrix());
            assert_eq!(transform.matrix(), Matrix3::<f32>::identity());
        }
        mat = mat * transform.invert().matrix();
        mat = mat * Matrix3::from_translation(Vector2::new(-0.5, -0.5));

        self.render_texture(texture, mat, alpha)
    }
}
