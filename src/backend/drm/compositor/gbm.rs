//! Implementation for ExportFramebuffer and related utilizies for gbm types

use std::os::unix::io::AsFd;

use super::{ExportBuffer, ExportFramebuffer};
use crate::backend::{
    allocator::gbm::GbmBuffer,
    drm::{
        gbm::{framebuffer_from_bo, framebuffer_from_wayland_buffer, Error, GbmFramebuffer},
        DrmDeviceFd,
    },
};

impl<A: AsFd + 'static> ExportFramebuffer<GbmBuffer> for gbm::Device<A> {
    type Framebuffer = GbmFramebuffer;
    type Error = Error;

    #[profiling::function]
    fn add_framebuffer(
        &self,
        drm: &DrmDeviceFd,
        buffer: ExportBuffer<'_, GbmBuffer>,
        use_opaque: bool,
    ) -> Result<Option<Self::Framebuffer>, Self::Error> {
        match buffer {
            #[cfg(feature = "wayland_frontend")]
            ExportBuffer::Wayland(wl_buffer) => {
                framebuffer_from_wayland_buffer(drm, self, wl_buffer, use_opaque)
            }
            ExportBuffer::Allocator(buffer) => framebuffer_from_bo(drm, buffer, use_opaque)
                .map_err(Error::Drm)
                .map(Some),
        }
    }

    #[inline]
    fn can_add_framebuffer(&self, buffer: &ExportBuffer<'_, GbmBuffer>) -> bool {
        match buffer {
            #[cfg(not(all(feature = "backend_egl", feature = "use_system_lib")))]
            ExportBuffer::Wayland(buffer) => matches!(
                crate::backend::renderer::buffer_type(buffer),
                Some(crate::backend::renderer::BufferType::Dma)
            ),
            #[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
            ExportBuffer::Wayland(buffer) => matches!(
                crate::backend::renderer::buffer_type(buffer),
                Some(crate::backend::renderer::BufferType::Dma)
                    | Some(crate::backend::renderer::BufferType::Egl)
            ),
            ExportBuffer::Allocator(_) => true,
        }
    }
}
