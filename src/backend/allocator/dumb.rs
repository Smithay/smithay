//! Module for [DumbBuffer](https://01.org/linuxgraphics/gfx-docs/drm/gpu/drm-kms.html#dumb-buffer-objects) buffers

use std::fmt;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;

use drm::buffer::Buffer as DrmBuffer;
use drm::control::{dumbbuffer::DumbBuffer as Handle, Device as ControlDevice};

use super::{Allocator, Buffer, Format};
use crate::backend::drm::device::{DrmDevice, DrmDeviceInternal, FdWrapper};

/// Wrapper around raw DumbBuffer handles.
pub struct DumbBuffer<A: AsRawFd + 'static> {
    fd: Arc<FdWrapper<A>>,
    handle: Handle,
    format: Format,
}

impl<A: AsRawFd + 'static> fmt::Debug for DumbBuffer<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DumbBuffer")
            .field("handle", &self.handle)
            .field("format", &self.format)
            .finish()
    }
}

impl<A: AsRawFd + 'static> Allocator<DumbBuffer<A>> for DrmDevice<A> {
    type Error = drm::SystemError;

    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        format: Format,
    ) -> Result<DumbBuffer<A>, Self::Error> {
        let handle = self.create_dumb_buffer((width, height), format.code, 32 /* TODO */)?;

        Ok(DumbBuffer {
            fd: match &*self.internal {
                DrmDeviceInternal::Atomic(dev) => dev.fd.clone(),
                DrmDeviceInternal::Legacy(dev) => dev.fd.clone(),
            },
            handle,
            format,
        })
    }
}

impl<A: AsRawFd + 'static> Buffer for DumbBuffer<A> {
    fn width(&self) -> u32 {
        self.handle.size().0
    }

    fn height(&self) -> u32 {
        self.handle.size().1
    }

    fn format(&self) -> Format {
        self.format
    }
}

impl<A: AsRawFd + 'static> DumbBuffer<A> {
    /// Raw handle to the underlying buffer.
    ///
    /// Note: This handle will become invalid, once the `DumbBuffer` wrapper is dropped
    /// or the device used to create is closed. Do not copy this handle and assume it keeps being valid.
    pub fn handle(&self) -> &Handle {
        &self.handle
    }
}

impl<A: AsRawFd + 'static> Drop for DumbBuffer<A> {
    fn drop(&mut self) {
        let _ = self.fd.destroy_dumb_buffer(self.handle);
    }
}
