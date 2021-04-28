use std::collections::HashSet;
use std::error::Error;

use cgmath::{prelude::*, Matrix3, Vector2};
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_shm, wl_buffer};

use crate::backend::SwapBuffersError;
#[cfg(feature = "renderer_gl")]
pub mod gles2;
#[cfg(feature = "wayland_frontend")]
use crate::backend::egl::EGLBuffer;

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum Transform {
    Normal,
    _90,
    _180,
    _270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl Transform {
    pub fn matrix(&self) -> Matrix3<f32> {
        match self {
	        Transform::Normal => Matrix3::new(
                1.0, 0.0, 0.0,
                0.0, 1.0, 0.0,
                0.0, 0.0, 1.0,
            ),
            Transform::_90 => Matrix3::new(
                0.0, -1.0, 0.0,
                1.0, 0.0, 0.0,
                0.0, 0.0, 1.0,
            ),
            Transform::_180 => Matrix3::new(
                -1.0, 0.0, 0.0,
                0.0, -1.0, 0.0,
                0.0, 0.0, 1.0,
            ),
            Transform::_270 => Matrix3::new(
                0.0, 1.0, 0.0,
                -1.0, 0.0, 0.0,
                0.0, 0.0, 1.0,
            ),
            Transform::Flipped => Matrix3::new(
                -1.0, 0.0, 0.0,
                0.0, 1.0, 0.0,
                0.0, 0.0, 1.0,
            ),
            Transform::Flipped90 => Matrix3::new(
                0.0, 1.0, 0.0,
                1.0, 0.0, 0.0,
                0.0, 0.0, 1.0,
            ),
            Transform::Flipped180 => Matrix3::new(
                1.0, 0.0, 0.0,
                0.0, -1.0, 0.0,
                0.0, 0.0, 1.0,
            ),
            Transform::Flipped270 => Matrix3::new(
                0.0, -1.0, 0.0,
                -1.0, 0.0, 0.0,
                0.0, 0.0, 1.0,
            ),
        }
    }

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

pub trait Bind<Target>: Unbind {
    fn bind(&mut self, target: Target) -> Result<(), <Self as Renderer>::Error>;
    fn supported_formats(&self) -> Option<HashSet<crate::backend::allocator::Format>> {
        None
    }
}

pub trait Unbind: Renderer {
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error>;
}

pub trait Texture {
    fn size(&self) -> (u32, u32) {
        (self.width(), self.height())
    }

    fn width(&self) -> u32;
    fn height(&self) -> u32;
}

pub trait Renderer {
    type Error: Error;
    type Texture: Texture;

    #[cfg(feature = "image")]
    fn import_bitmap<C: std::ops::Deref<Target=[u8]>>(&mut self, image: &image::ImageBuffer<image::Rgba<u8>, C>) -> Result<Self::Texture, Self::Error>; 
    #[cfg(feature = "wayland_frontend")]
    fn shm_formats(&self) -> &[wl_shm::Format] {
        // Mandatory
        &[wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888]
    }
    #[cfg(feature = "wayland_frontend")]
    fn import_shm(&mut self, buffer: &wl_buffer::WlBuffer) -> Result<Self::Texture, Self::Error>;
    #[cfg(feature = "wayland_frontend")]
    fn import_egl(&mut self, buffer: &EGLBuffer) -> Result<Self::Texture, Self::Error>;
    fn destroy_texture(&mut self, texture: Self::Texture) -> Result<(), Self::Error>;

    fn begin(&mut self, width: u32, height: u32, transform: Transform) -> Result<(), <Self as Renderer>::Error>;
    fn finish(&mut self) -> Result<(), SwapBuffersError>;
    
    fn clear(&mut self, color: [f32; 4]) -> Result<(), Self::Error>;
    fn render_texture(&mut self, texture: &Self::Texture, matrix: Matrix3<f32>, alpha: f32) -> Result<(), Self::Error>;
    fn render_texture_at(&mut self, texture: &Self::Texture, pos: (i32, i32), transform: Transform, alpha: f32) -> Result<(), Self::Error> {
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