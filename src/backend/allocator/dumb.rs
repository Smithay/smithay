use std::os::unix::io::AsRawFd;
use std::sync::Arc;

use drm::buffer::Buffer as DrmBuffer;
use drm::control::{dumbbuffer::DumbBuffer as Handle, Device as ControlDevice};

use super::{Allocator, Buffer, Format};
use crate::backend::drm::device::{DrmDevice, DrmDeviceInternal, FdWrapper};

pub struct DumbBuffer<A: AsRawFd + 'static> {
    fd: Arc<FdWrapper<A>>,
    pub handle: Handle,
    format: Format,
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

impl<A: AsRawFd + 'static> Drop for DumbBuffer<A> {
    fn drop(&mut self) {
        let _ = self.fd.destroy_dumb_buffer(self.handle);
    }
}
