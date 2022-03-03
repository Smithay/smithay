use std::cell::Cell;

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        renderer::{Frame, ImportDma, ImportDmaWl, ImportMem, ImportMemWl, Renderer, Texture, TextureFilter},
        SwapBuffersError,
    },
    reexports::wayland_server::protocol::wl_buffer,
    utils::{Buffer, Physical, Rectangle, Size, Transform},
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

    fn id(&self) -> usize {
        0
    }

    fn render<F, R>(
        &mut self,
        _size: Size<i32, Physical>,
        _dst_transform: Transform,
        rendering: F,
    ) -> Result<R, Self::Error>
    where
        F: FnOnce(&mut Self, &mut Self::Frame) -> R,
    {
        let mut frame = DummyFrame {};
        Ok(rendering(self, &mut frame))
    }

    fn upscale_filter(&mut self, _filter: TextureFilter) -> Result<(), Self::Error> {
        Ok(())
    }

    fn downscale_filter(&mut self, _filter: TextureFilter) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl ImportMem for DummyRenderer {
    fn import_memory(
        &mut self,
        _data: &[u8],
        _size: Size<i32, Buffer>,
        _flipped: bool,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        unimplemented!()
    }

    fn update_memory(
        &mut self,
        _texture: &<Self as Renderer>::TextureId,
        _data: &[u8],
        _region: Rectangle<i32, Buffer>,
    ) -> Result<(), <Self as Renderer>::Error> {
        unimplemented!()
    }
}

impl ImportMemWl for DummyRenderer {
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
        _damage: Option<&[Rectangle<i32, Buffer>]>,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        unimplemented!()
    }
}

impl ImportDmaWl for DummyRenderer {}

pub struct DummyFrame {}

impl Frame for DummyFrame {
    type Error = SwapBuffersError;
    type TextureId = DummyTexture;

    fn clear(&mut self, _color: [f32; 4], _damage: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
        Ok(())
    }

    fn render_texture_from_to(
        &mut self,
        _texture: &Self::TextureId,
        _src: Rectangle<i32, Buffer>,
        _dst: Rectangle<f64, Physical>,
        _damage: &[Rectangle<i32, Buffer>],
        _src_transform: Transform,
        _alpha: f32,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn transformation(&self) -> Transform {
        Transform::Normal
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
