//! Utilities to attach [`framebuffer::Handle`]s to gbm backed buffers

use std::os::unix::io::AsFd;

use thiserror::Error;

use drm::{
    buffer::PlanarBuffer,
    control::{framebuffer, Device, FbCmd2Flags},
};
use drm_fourcc::DrmModifier;
use tracing::{trace, warn};
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::wl_buffer::WlBuffer;

#[cfg(feature = "wayland_frontend")]
use crate::backend::allocator::Buffer;
use crate::backend::{
    allocator::{
        dmabuf::Dmabuf,
        format::{get_bpp, get_depth, get_opaque},
        gbm::GbmBuffer,
        Fourcc,
    },
    drm::DrmDeviceFd,
};
use crate::utils::DevPath;

use super::{error::AccessError, warn_legacy_fb_export, Framebuffer};

/// A GBM backed framebuffer
#[derive(Debug)]
pub struct GbmFramebuffer {
    fb: framebuffer::Handle,
    format: drm_fourcc::DrmFormat,
    drm: DrmDeviceFd,
}

impl Drop for GbmFramebuffer {
    #[inline]
    fn drop(&mut self) {
        trace!(fb = ?self.fb, "destroying framebuffer");
        if let Err(err) = self.drm.destroy_framebuffer(self.fb) {
            warn!(fb = ?self.fb, ?err, "failed to destroy framebuffer");
        }
    }
}

impl AsRef<framebuffer::Handle> for GbmFramebuffer {
    #[inline]
    fn as_ref(&self) -> &framebuffer::Handle {
        &self.fb
    }
}

impl Framebuffer for GbmFramebuffer {
    #[inline]
    fn format(&self) -> drm_fourcc::DrmFormat {
        self.format
    }
}

/// Attach a framebuffer for a [`WlBuffer`]
///
/// This tries to import the buffer to gbm and attach a [`framebuffer::Handle`] for
/// the imported [`BufferObject`][gbm::BufferObject].
///
/// Returns `Ok(None)` for unknown buffer types and buffer types that do not
/// support attaching a framebuffer (e.g. shm-buffers)
#[cfg(feature = "wayland_frontend")]
#[profiling::function]
pub fn framebuffer_from_wayland_buffer<A: AsFd + 'static>(
    drm: &DrmDeviceFd,
    gbm: &gbm::Device<A>,
    buffer: &WlBuffer,
    use_opaque: bool,
) -> Result<Option<GbmFramebuffer>, Error> {
    if let Ok(dmabuf) = crate::wayland::dmabuf::get_dmabuf(buffer) {
        // From weston:
        /* We should not import to KMS a buffer that has been allocated using no
         * modifiers. Usually drivers use linear layouts to allocate with no
         * modifiers, but this is not a rule. The driver could use, for
         * instance, a tiling layout under the hood - and both Weston and the
         * KMS driver can't know. So giving the buffer to KMS is not safe, as
         * not knowing its layout can result in garbage being displayed. In
         * short, importing a buffer to KMS requires explicit modifiers. */
        if dmabuf.format().modifier != DrmModifier::Invalid {
            return Ok(Some(framebuffer_from_dmabuf(
                drm, gbm, dmabuf, use_opaque, false,
            )?));
        }
    }

    #[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
    if matches!(
        crate::backend::renderer::buffer_type(buffer),
        Some(crate::backend::renderer::BufferType::Egl)
    ) {
        let bo = gbm
            .import_buffer_object_from_wayland::<()>(buffer, gbm::BufferObjectFlags::SCANOUT)
            .map(|bo| GbmBuffer::from_bo(bo, true))
            .map_err(Error::Import)?;
        let (fb, format) = framebuffer_from_bo_internal(
            drm,
            BufferObjectInternal {
                bo: &bo,
                offsets: None,
                pitches: None,
            },
            use_opaque,
            true,
        )
        .map_err(Error::Drm)?;

        return Ok(Some(GbmFramebuffer {
            fb,
            format,
            drm: drm.clone(),
        }));
    }

    Ok(None)
}

/// Possible errors for attaching a [`framebuffer::Handle`]
#[derive(Error, Debug)]
pub enum Error {
    /// Importing the [`Dmabuf`] to gbm failed
    #[error("failed to import the dmabuf to gbm")]
    Import(std::io::Error),
    /// Failed to add a framebuffer for the bo
    #[error("failed to add a framebuffer for the bo")]
    Drm(AccessError),
}

/// Attach a framebuffer for a [`Dmabuf`]
///
/// This tries to import the [`Dmabuf`] using gbm and attach
/// a [`framebuffer::Handle`] for the imported [`BufferObject`][gbm::BufferObject]
#[profiling::function]
pub fn framebuffer_from_dmabuf<A: AsFd + 'static>(
    drm: &DrmDeviceFd,
    gbm: &gbm::Device<A>,
    dmabuf: &Dmabuf,
    use_opaque: bool,
    allow_legacy: bool,
) -> Result<GbmFramebuffer, Error> {
    let bo: GbmBuffer = dmabuf
        .import_to(gbm, gbm::BufferObjectFlags::SCANOUT)
        .map_err(Error::Import)?;

    // We override the offsets and pitches here cause the imported bo
    // can return the wrong values. bo will only return the correct values
    // for buffers we have allocated, but not for all client provided buffers.
    let mut offsets: [u32; 4] = [0; 4];
    let mut pitches: [u32; 4] = [0; 4];

    for (index, offset) in dmabuf.offsets().enumerate() {
        offsets[index] = offset;
    }

    for (index, stride) in dmabuf.strides().enumerate() {
        pitches[index] = stride;
    }

    framebuffer_from_bo_internal(
        drm,
        BufferObjectInternal {
            bo: &bo,
            offsets: Some(offsets),
            pitches: Some(pitches),
        },
        use_opaque,
        allow_legacy,
    )
    .map_err(Error::Drm)
    .map(|(fb, format)| GbmFramebuffer {
        fb,
        format,
        drm: drm.clone(),
    })
}

/// Attach a [`framebuffer::Handle`] to an [`BufferObject`][gbm::BufferObject]
#[profiling::function]
pub fn framebuffer_from_bo(
    drm: &DrmDeviceFd,
    bo: &GbmBuffer,
    use_opaque: bool,
) -> Result<GbmFramebuffer, AccessError> {
    framebuffer_from_bo_internal(
        drm,
        BufferObjectInternal {
            bo,
            offsets: None,
            pitches: None,
        },
        use_opaque,
        true,
    )
    .map(|(fb, format)| GbmFramebuffer {
        fb,
        format,
        drm: drm.clone(),
    })
}

struct BufferObjectInternal<'a> {
    bo: &'a GbmBuffer,
    pitches: Option<[u32; 4]>,
    offsets: Option<[u32; 4]>,
}

impl std::ops::Deref for BufferObjectInternal<'_> {
    type Target = GbmBuffer;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.bo
    }
}

impl PlanarBuffer for BufferObjectInternal<'_> {
    #[inline]
    fn size(&self) -> (u32, u32) {
        PlanarBuffer::size(self.bo)
    }

    #[inline]
    fn format(&self) -> drm_fourcc::DrmFourcc {
        PlanarBuffer::format(self.bo)
    }

    #[inline]
    fn modifier(&self) -> Option<DrmModifier> {
        PlanarBuffer::modifier(self.bo)
    }

    #[inline]
    fn pitches(&self) -> [u32; 4] {
        self.pitches.unwrap_or_else(|| PlanarBuffer::pitches(self.bo))
    }

    #[inline]
    fn handles(&self) -> [Option<drm::buffer::Handle>; 4] {
        PlanarBuffer::handles(self.bo)
    }

    #[inline]
    fn offsets(&self) -> [u32; 4] {
        self.offsets.unwrap_or_else(|| PlanarBuffer::offsets(self.bo))
    }
}

struct OpaqueBufferWrapper<'a, B>(&'a B);
impl<B> PlanarBuffer for OpaqueBufferWrapper<'_, B>
where
    B: PlanarBuffer,
{
    #[inline]
    fn size(&self) -> (u32, u32) {
        self.0.size()
    }

    #[inline]
    fn format(&self) -> Fourcc {
        let fmt = self.0.format();
        get_opaque(fmt).unwrap_or(fmt)
    }

    #[inline]
    fn modifier(&self) -> Option<DrmModifier> {
        self.0.modifier()
    }

    #[inline]
    fn pitches(&self) -> [u32; 4] {
        self.0.pitches()
    }

    #[inline]
    fn handles(&self) -> [Option<drm::buffer::Handle>; 4] {
        self.0.handles()
    }

    #[inline]
    fn offsets(&self) -> [u32; 4] {
        self.0.offsets()
    }
}

#[profiling::function]
#[inline]
fn framebuffer_from_bo_internal<D>(
    drm: &D,
    bo: BufferObjectInternal<'_>,
    use_opaque: bool,
    allow_legacy: bool,
) -> Result<(framebuffer::Handle, drm_fourcc::DrmFormat), AccessError>
where
    D: drm::control::Device + DevPath,
{
    let modifier = bo.modifier();
    let flags = if bo.modifier().is_some() {
        FbCmd2Flags::MODIFIERS
    } else {
        FbCmd2Flags::empty()
    };

    let ret = if use_opaque {
        let opaque_wrapper = OpaqueBufferWrapper(&bo);
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
        drm.add_planar_framebuffer(&bo, flags).map(|fb| {
            (
                fb,
                drm_fourcc::DrmFormat {
                    code: bo.format(),
                    modifier: modifier.unwrap_or(DrmModifier::Invalid),
                },
            )
        })
    };

    let (fb, format) = match ret {
        Ok(fb) => fb,
        Err(source) => {
            if !allow_legacy {
                return Err(AccessError {
                    errmsg: "Failed to add framebuffer",
                    dev: drm.dev_path(),
                    source,
                });
            }

            // We only support this as a fallback of last resort like xf86-video-modesetting does.
            warn_legacy_fb_export();

            if bo.plane_count() > 1 {
                return Err(AccessError {
                    errmsg: "Failed to add framebuffer",
                    dev: drm.dev_path(),
                    source,
                });
            }

            let fourcc = bo.format();
            let (depth, bpp) = get_depth(fourcc)
                .and_then(|d| get_bpp(fourcc).map(|b| (d, b)))
                .ok_or_else(|| AccessError {
                    errmsg: "Unknown format for legacy framebuffer",
                    dev: drm.dev_path(),
                    source,
                })?;

            let fb = drm
                .add_framebuffer(&*bo, depth as u32, bpp as u32)
                .map_err(|source| AccessError {
                    errmsg: "Failed to add framebuffer",
                    dev: drm.dev_path(),
                    source,
                })?;
            (
                fb,
                drm_fourcc::DrmFormat {
                    code: fourcc,
                    modifier: drm_fourcc::DrmModifier::Invalid,
                },
            )
        }
    };
    Ok((fb, format))
}
