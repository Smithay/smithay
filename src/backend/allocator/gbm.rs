//! Module for Buffers created using [libgbm](gbm).
//!
//! The re-exported [`GbmDevice`](gbm::Device) implements the [`Allocator`](super::Allocator) trait
//! and [`GbmBuffer`](gbm::BufferObject) satisfies the [`Buffer`](super::Buffer) trait while also allowing
//! conversions to and from [dmabufs](super::dmabuf).

use super::{
    dmabuf::{AsDmabuf, Dmabuf, DmabufFlags, MAX_PLANES},
    Allocator, Buffer, Format, Fourcc, Modifier,
};
use crate::utils::{Buffer as BufferCoords, Size};
pub use gbm::{BufferObject as GbmBuffer, BufferObjectFlags as GbmBufferFlags, Device as GbmDevice};
use std::os::unix::io::AsRawFd;

impl<A: AsRawFd + 'static, T> Allocator<GbmBuffer<T>> for GbmDevice<A> {
    type Error = std::io::Error;

    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: &[Modifier],
    ) -> Result<GbmBuffer<T>, Self::Error> {
        match self.create_buffer_object_with_modifiers(width, height, fourcc, modifiers.iter().copied()) {
            Ok(bo) => Ok(bo),
            Err(err) => {
                if modifiers.contains(&Modifier::Invalid) || modifiers.contains(&Modifier::Linear) {
                    let mut usage = GbmBufferFlags::SCANOUT | GbmBufferFlags::RENDERING;
                    if !modifiers.contains(&Modifier::Invalid) {
                        usage |= GbmBufferFlags::LINEAR;
                    }
                    self.create_buffer_object(width, height, fourcc, usage)
                } else {
                    Err(err)
                }
            }
        }
    }
}

impl<T> Buffer for GbmBuffer<T> {
    fn size(&self) -> Size<i32, BufferCoords> {
        (
            self.width().unwrap_or(0) as i32,
            self.height().unwrap_or(0) as i32,
        )
            .into()
    }

    fn format(&self) -> Format {
        Format {
            code: self.format().unwrap_or(Fourcc::Argb8888), // we got to return something, but this should never happen anyway
            modifier: self.modifier().unwrap_or(Modifier::Invalid),
        }
    }
}

/// Errors during conversion to a dmabuf handle from a gbm buffer object
#[derive(thiserror::Error, Debug)]
pub enum GbmConvertError {
    /// The gbm device was destroyed
    #[error("The gbm device was destroyed")]
    DeviceDestroyed(#[from] gbm::DeviceDestroyedError),
    /// The buffer consists out of multiple file descriptions, which is currently unsupported
    #[error("Buffer consists out of multiple file descriptors, which is currently unsupported")]
    UnsupportedBuffer,
    /// The conversion returned an invalid file descriptor
    #[error("Buffer returned invalid file descriptor")]
    InvalidFD,
}

impl<T> AsDmabuf for GbmBuffer<T> {
    type Error = GbmConvertError;

    fn export(&self) -> Result<Dmabuf, GbmConvertError> {
        let planes = self.plane_count()? as i32;

        //TODO switch to gbm_bo_get_plane_fd when it lands
        let mut iter = (0i32..planes).map(|i| self.handle_for_plane(i));
        let first = iter.next().expect("Encountered a buffer with zero planes");
        // check that all handles are the same
        let handle = iter.try_fold(first, |first, next| {
            if let (Ok(next), Ok(first)) = (next, first) {
                if unsafe { next.u64_ == first.u64_ } {
                    return Some(Ok(first));
                }
            }
            None
        });
        if handle.is_none() {
            // GBM is lacking a function to get a FD for a given plane. Instead,
            // check all planes have the same handle. We can't use
            // drmPrimeHandleToFD because that messes up handle ref'counting in
            // the user-space driver.
            return Err(GbmConvertError::UnsupportedBuffer); //TODO
        }

        // Make sure to only call fd once as each call will create
        // a new file descriptor which has to be closed
        let fd = self.fd()?;

        // gbm_bo_get_fd returns -1 if an error occurs
        if fd == -1 {
            return Err(GbmConvertError::InvalidFD);
        }

        let mut builder = Dmabuf::builder_from_buffer(self, DmabufFlags::empty());
        for idx in 0..planes {
            builder.add_plane(
                fd,
                idx as u32,
                self.offset(idx)?,
                self.stride_for_plane(idx)?,
                self.modifier()?,
            );
        }
        Ok(builder.build().unwrap())
    }
}

impl Dmabuf {
    /// Import a Dmabuf using libgbm, creating a gbm Buffer Object to the same underlying data.
    pub fn import_to<A: AsRawFd + 'static, T>(
        &self,
        gbm: &GbmDevice<A>,
        usage: GbmBufferFlags,
    ) -> std::io::Result<GbmBuffer<T>> {
        let mut handles = [0; MAX_PLANES];
        for (i, handle) in self.handles().take(MAX_PLANES).enumerate() {
            handles[i] = handle;
        }
        let mut strides = [0i32; MAX_PLANES];
        for (i, stride) in self.strides().take(MAX_PLANES).enumerate() {
            strides[i] = stride as i32;
        }
        let mut offsets = [0i32; MAX_PLANES];
        for (i, offset) in self.offsets().take(MAX_PLANES).enumerate() {
            offsets[i] = offset as i32;
        }

        if self.has_modifier() || self.num_planes() > 1 || self.offsets().next().unwrap() != 0 {
            gbm.import_buffer_object_from_dma_buf_with_modifiers(
                self.num_planes() as u32,
                handles,
                self.width(),
                self.height(),
                self.format().code,
                usage,
                strides,
                offsets,
                self.format().modifier,
            )
        } else {
            gbm.import_buffer_object_from_dma_buf(
                handles[0],
                self.width(),
                self.height(),
                strides[0] as u32,
                self.format().code,
                if self.format().modifier == Modifier::Linear {
                    usage | GbmBufferFlags::LINEAR
                } else {
                    usage
                },
            )
        }
    }
}
