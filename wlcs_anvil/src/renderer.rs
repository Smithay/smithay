use std::cell::Cell;

use cgmath::Vector2;
use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        renderer::{Frame, ImportDma, ImportShm, Renderer, Texture, Transform},
        SwapBuffersError,
    },
    reexports::wayland_server::protocol::wl_buffer,
    utils::{Buffer, Physical, Rectangle, Size},
    wayland::compositor::SurfaceData,
};

pub struct DummyRenderer {}

impl DummyRenderer {
    pub fn new() -> DummyRenderer {
        DummyRenderer {}
    }
}

impl Renderer for DummyRenderer {
    type Error = SwapBuffersError;
    type TextureId = DummyTexture;
    type Frame = DummyFrame;

    fn render<F, R>(
        &mut self,
        _size: Size<i32, Physical>,
        _transform: Transform,
        rendering: F,
    ) -> Result<R, Self::Error>
    where
        F: FnOnce(&mut Self, &mut Self::Frame) -> R,
    {
        let mut frame = DummyFrame {};
        Ok(rendering(self, &mut frame))
    }
}

impl ImportShm for DummyRenderer {
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceData>,
        _damage: &[Rectangle<i32, Buffer>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        use smithay::wayland::shm::with_buffer_contents;
        let ret = with_buffer_contents(&buffer, |slice, data| {
            let offset = data.offset as u32;
            let width = data.width as u32;
            let height = data.height as u32;
            let stride = data.stride as u32;

            let mut x = 0;
            for h in 0..height {
                for w in 0..width {
                    x |= slice[(offset + w + h * stride) as usize];
                }
            }

            if let Some(data) = surface {
                data.data_map.insert_if_missing(|| Cell::new(0u8));
                data.data_map.get::<Cell<u8>>().unwrap().set(x);
            }

            (width, height)
        });

        match ret {
            Ok((width, height)) => Ok(DummyTexture { width, height }),
            Err(e) => Err(SwapBuffersError::TemporaryFailure(Box::new(e))),
        }
    }
}

impl ImportDma for DummyRenderer {
    fn import_dmabuf(
        &mut self,
        _dmabuf: &Dmabuf,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        unimplemented!()
    }
}

pub struct DummyFrame {}

impl Frame for DummyFrame {
    type Error = SwapBuffersError;
    type TextureId = DummyTexture;

    fn clear(&mut self, _color: [f32; 4]) -> Result<(), Self::Error> {
        Ok(())
    }

    fn render_texture(
        &mut self,
        _texture: &Self::TextureId,
        _matrix: cgmath::Matrix3<f32>,
        _tex_coords: [Vector2<f32>; 4],
        _alpha: f32,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub struct DummyTexture {
    width: u32,
    height: u32,
}

impl Texture for DummyTexture {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }
}
