//! Implementation for ExportFramebuffer and related utilizies for gbm types

use std::os::unix::io::AsFd;

use gbm::BufferObject;

use super::{ExportBuffer, ExportFramebuffer};
use crate::backend::drm::{
    gbm::{framebuffer_from_bo, framebuffer_from_wayland_buffer, Error, GbmFramebuffer},
    DrmDeviceFd,
};

impl<A: AsFd + 'static, T> ExportFramebuffer<BufferObject<T>> for gbm::Device<A> {
    type Framebuffer = GbmFramebuffer;
    type Error = Error;

    fn add_framebuffer(
        &self,
        drm: &DrmDeviceFd,
        buffer: ExportBuffer<'_, BufferObject<T>>,
        allow_opaque_fallback: bool,
    ) -> Result<Option<Self::Framebuffer>, Self::Error> {
        match buffer {
            #[cfg(feature = "wayland_frontend")]
            ExportBuffer::Wayland(wl_buffer) => {
                framebuffer_from_wayland_buffer(drm, self, wl_buffer, allow_opaque_fallback)
            }
            ExportBuffer::Allocator(buffer) => framebuffer_from_bo(drm, buffer, allow_opaque_fallback)
                .map_err(Error::Drm)
                .map(Some),
        }
    }
}
