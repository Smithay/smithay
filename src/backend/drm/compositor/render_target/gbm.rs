use crate::backend::{
    allocator::{
        dmabuf::{AsDmabuf, Dmabuf},
        gbm::{GbmBuffer, GbmConvertError},
        Slot,
    },
    renderer::{Bind, Renderer},
};

use super::{AsRenderTarget, DefaultAsRenderTarget};

impl<R> AsRenderTarget<GbmBuffer, R> for DefaultAsRenderTarget
where
    R: Renderer + Bind<Dmabuf>,
{
    type Target<'buffer> = Dmabuf;
    type Error = GbmConvertError;

    fn as_render_target<'buffer>(
        _renderer: &mut R,
        slot: &'buffer mut Slot<GbmBuffer>,
    ) -> Result<Self::Target<'buffer>, Self::Error> {
        slot.export()
    }
}
