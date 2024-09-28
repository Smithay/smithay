//! Module for [DumbBuffer](https://docs.kernel.org/gpu/drm-kms.html#dumb-buffer-objects) buffers

use std::fmt;
use std::io;

use drm::buffer::Buffer as DrmBuffer;
use drm::control::{dumbbuffer::DumbBuffer as Handle, Device as ControlDevice};
use tracing::instrument;

use super::dmabuf::{AsDmabuf, Dmabuf, DmabufFlags};
use super::{format::get_bpp, Allocator, Buffer, Format, Fourcc, Modifier};
use crate::backend::drm::DrmDeviceFd;
use crate::backend::drm::DrmNode;
use crate::utils::{Buffer as BufferCoords, Size};

/// Wrapper around raw DumbBuffer handles.
pub struct DumbBuffer {
    fd: DrmDeviceFd,
    handle: Handle,
    format: Format,
}

impl fmt::Debug for DumbBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DumbBuffer")
            .field("handle", &self.handle)
            .field("format", &self.format)
            .finish()
    }
}

/// Light wrapper around an [`DrmDeviceFd`] to implement the [`Allocator`]-trait
#[derive(Debug)]
pub struct DumbAllocator {
    fd: DrmDeviceFd,
}

impl DumbAllocator {
    /// Create a new [`DumbAllocator`] from a [`DrmDeviceFd`].
    pub fn new(fd: DrmDeviceFd) -> Self {
        DumbAllocator { fd }
    }
}

impl Allocator for DumbAllocator {
    type Buffer = DumbBuffer;
    type Error = io::Error;

    #[instrument(level = "trace", err)]
    #[profiling::function]
    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: &[Modifier],
    ) -> Result<DumbBuffer, Self::Error> {
        // dumb buffers are always linear
        if modifiers
            .iter()
            .all(|&x| x != Modifier::Invalid && x != Modifier::Linear)
        {
            return Err(rustix::io::Errno::INVAL.into());
        }

        let handle = self.fd.create_dumb_buffer(
            (width, height),
            fourcc,
            get_bpp(fourcc).ok_or(rustix::io::Errno::INVAL)? as u32,
        )?;

        Ok(DumbBuffer {
            fd: self.fd.clone(),
            handle,
            format: Format {
                code: fourcc,
                modifier: Modifier::Linear,
            },
        })
    }
}

impl Buffer for DumbBuffer {
    fn size(&self) -> Size<i32, BufferCoords> {
        let (w, h) = self.handle.size();
        (w as i32, h as i32).into()
    }

    fn format(&self) -> Format {
        self.format
    }
}

impl DumbBuffer {
    /// Raw handle to the underlying buffer.
    ///
    /// Note: This handle will become invalid, once the `DumbBuffer` wrapper is dropped
    /// or the device used to create is closed. Do not copy this handle and assume it keeps being valid.
    pub fn handle(&self) -> &Handle {
        &self.handle
    }
}

impl AsDmabuf for DumbBuffer {
    type Error = io::Error;

    #[profiling::function]
    fn export(&self) -> Result<Dmabuf, Self::Error> {
        let fd = self
            .fd
            .buffer_to_prime_fd(self.handle.handle(), drm::CLOEXEC | drm::RDWR)?;
        let mut builder = Dmabuf::builder(
            self.size(),
            self.format.code,
            self.format.modifier,
            DmabufFlags::empty(),
        );
        builder.add_plane(fd, 0, 0, self.handle.pitch());
        if let Ok(node) = DrmNode::from_file(&self.fd) {
            builder.set_node(node);
        }
        builder.build().ok_or(rustix::io::Errno::INVAL.into())
    }
}

impl Drop for DumbBuffer {
    fn drop(&mut self) {
        let _ = self.fd.destroy_dumb_buffer(self.handle);
    }
}
