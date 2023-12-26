//! Utilities to attach [`framebuffer::Handle`]s to dumb buffers

use drm_fourcc::{DrmFormat, DrmModifier};

use drm::{
    buffer::{Buffer as DrmBuffer, PlanarBuffer},
    control::{framebuffer, Device, FbCmd2Flags},
};
use tracing::{trace, warn};

use crate::utils::DevPath;
use crate::{
    backend::{
        allocator::{
            dumb::DumbBuffer,
            format::{get_bpp, get_depth, get_opaque},
            Buffer, Fourcc,
        },
        drm::DrmDeviceFd,
    },
    utils::{Buffer as BufferCoords, Size},
};

use super::{error::AccessError, warn_legacy_fb_export, Framebuffer};

/// A GBM backed framebuffer
#[derive(Debug)]
pub struct DumbFramebuffer {
    fb: framebuffer::Handle,
    format: drm_fourcc::DrmFormat,
    drm: DrmDeviceFd,
}

impl Drop for DumbFramebuffer {
    fn drop(&mut self) {
        trace!(fb = ?self.fb, "destroying framebuffer");
        if let Err(err) = self.drm.destroy_framebuffer(self.fb) {
            warn!(fb = ?self.fb, ?err, "failed to destroy framebuffer");
        }
    }
}

impl AsRef<framebuffer::Handle> for DumbFramebuffer {
    fn as_ref(&self) -> &framebuffer::Handle {
        &self.fb
    }
}

impl Framebuffer for DumbFramebuffer {
    fn format(&self) -> drm_fourcc::DrmFormat {
        self.format
    }
}

struct PlanarDumbBuffer<'a>(&'a DumbBuffer);

impl<'a> Buffer for PlanarDumbBuffer<'a> {
    fn size(&self) -> Size<i32, BufferCoords> {
        self.0.size()
    }

    fn format(&self) -> DrmFormat {
        self.0.format()
    }
}

impl<'a> PlanarBuffer for PlanarDumbBuffer<'a> {
    fn size(&self) -> (u32, u32) {
        let size = self.0.size();
        (size.w as u32, size.h as u32)
    }

    fn format(&self) -> Fourcc {
        self.0.format().code
    }

    fn modifier(&self) -> Option<DrmModifier> {
        Some(self.0.format().modifier)
    }

    fn pitches(&self) -> [u32; 4] {
        [self.0.handle().pitch(), 0, 0, 0]
    }

    fn handles(&self) -> [Option<drm::buffer::Handle>; 4] {
        [Some(self.0.handle().handle()), None, None, None]
    }

    fn offsets(&self) -> [u32; 4] {
        [0, 0, 0, 0]
    }
}

struct OpaqueBufferWrapper<'a, B>(&'a B);
impl<'a, B> PlanarBuffer for OpaqueBufferWrapper<'a, B>
where
    B: PlanarBuffer,
{
    fn size(&self) -> (u32, u32) {
        self.0.size()
    }

    fn format(&self) -> Fourcc {
        let fmt = self.0.format();
        get_opaque(fmt).unwrap_or(fmt)
    }

    fn modifier(&self) -> Option<DrmModifier> {
        self.0.modifier()
    }

    fn pitches(&self) -> [u32; 4] {
        self.0.pitches()
    }

    fn handles(&self) -> [Option<drm::buffer::Handle>; 4] {
        self.0.handles()
    }

    fn offsets(&self) -> [u32; 4] {
        self.0.offsets()
    }
}

/// Attach a [`framebuffer::Handle`] to an [`DumbBuffer`]
#[profiling::function]
pub fn framebuffer_from_dumb_buffer(
    drm: &DrmDeviceFd,
    buffer: &DumbBuffer,
    use_opaque: bool,
) -> Result<DumbFramebuffer, AccessError> {
    let buffer = PlanarDumbBuffer(buffer);

    let format = Buffer::format(&buffer);
    let modifier = buffer.modifier();
    let flags = if modifier.is_some() {
        FbCmd2Flags::MODIFIERS
    } else {
        FbCmd2Flags::empty()
    };

    let ret = if use_opaque {
        let opaque_wrapper = OpaqueBufferWrapper(&buffer);
        drm.add_planar_framebuffer(&opaque_wrapper, flags).map(|fb| {
            (
                fb,
                drm_fourcc::DrmFormat {
                    code: opaque_wrapper.format(),
                    modifier: modifier.unwrap_or(DrmModifier::Invalid),
                },
            )
        })
    } else {
        drm.add_planar_framebuffer(&buffer, flags).map(|fb| (fb, format))
    };

    let (fb, format) = match ret {
        Ok(fb) => fb,
        Err(source) => {
            warn_legacy_fb_export();

            let fourcc = format.code;
            let (depth, bpp) = get_depth(fourcc)
                .and_then(|d| get_bpp(fourcc).map(|b| (d, b)))
                .ok_or_else(|| AccessError {
                    errmsg: "Unknown format for legacy framebuffer",
                    dev: drm.dev_path(),
                    source,
                })?;

            let fb = drm
                .add_framebuffer(buffer.0.handle(), depth as u32, bpp as u32)
                .map_err(|source| AccessError {
                    errmsg: "Failed to add framebuffer",
                    dev: drm.dev_path(),
                    source,
                })?;
            (fb, format)
        }
    };

    Ok(DumbFramebuffer {
        fb,
        format,
        drm: drm.clone(),
    })
}
