//! Utilities to attach [`framebuffer::Handle`]s to gbm backed buffers

use std::os::unix::io::AsFd;
use std::path::PathBuf;

use thiserror::Error;

use drm::{
    buffer::PlanarBuffer,
    control::{framebuffer, Device},
};
#[cfg(feature = "wayland_frontend")]
use drm_fourcc::DrmModifier;
use gbm::BufferObject;
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::wl_buffer::WlBuffer;

use super::{ExportBuffer, ExportFramebuffer};
use crate::backend::{
    allocator::{
        dmabuf::Dmabuf,
        format::{get_bpp, get_depth},
        Buffer,
    },
    drm::DrmDeviceFd,
};
use crate::utils::DevPath;

/// A GBM backed framebuffer
#[derive(Debug)]
pub struct GbmFramebuffer {
    _bo: Option<BufferObject<()>>,
    fb: framebuffer::Handle,
    drm: DrmDeviceFd,
}

impl Drop for GbmFramebuffer {
    fn drop(&mut self) {
        let _ = self.drm.destroy_framebuffer(self.fb);
    }
}

impl AsRef<framebuffer::Handle> for GbmFramebuffer {
    fn as_ref(&self) -> &framebuffer::Handle {
        &self.fb
    }
}

impl<A: AsFd + 'static, T> ExportFramebuffer<BufferObject<T>> for gbm::Device<A> {
    type Framebuffer = GbmFramebuffer;
    type Error = Error;

    fn add_framebuffer(
        &self,
        drm: &DrmDeviceFd,
        buffer: ExportBuffer<'_, BufferObject<T>>,
    ) -> Result<Option<Self::Framebuffer>, Self::Error> {
        match buffer {
            #[cfg(feature = "wayland_frontend")]
            ExportBuffer::Wayland(wl_buffer) => framebuffer_from_wayland_buffer(drm, self, wl_buffer),
            ExportBuffer::Allocator(buffer) => framebuffer_from_bo(drm, buffer).map_err(Error::Drm).map(Some),
        }
    }
}

/// Attach a framebuffer for a [`WlBuffer`]
///
/// This tries to import the buffer to gbm and attach a [`framebuffer::Handle`] for
/// the imported [`BufferObject`].
///
/// Returns `Ok(None)` for unknown buffer types and buffer types that do not
/// support attaching a framebuffer (e.g. shm-buffers)
#[cfg(feature = "wayland_frontend")]
pub fn framebuffer_from_wayland_buffer<A: AsFd + 'static>(
    drm: &DrmDeviceFd,
    gbm: &gbm::Device<A>,
    buffer: &WlBuffer,
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
            return Ok(Some(framebuffer_from_dmabuf(drm, gbm, &dmabuf)?));
        }
    }

    #[cfg(all(feature = "backend_egl", feature = "use_system_lib"))]
    if matches!(
        crate::backend::renderer::buffer_type(buffer),
        Some(crate::backend::renderer::BufferType::Egl)
    ) {
        let bo = gbm
            .import_buffer_object_from_wayland::<()>(buffer, gbm::BufferObjectFlags::SCANOUT)
            .map_err(Error::Import)?;
        let fb = framebuffer_from_bo_internal(
            drm,
            BufferObjectInternal {
                bo: &bo,
                offsets: None,
                pitches: None,
            },
        )
        .map_err(Error::Drm)?;

        return Ok(Some(GbmFramebuffer {
            _bo: Some(bo),
            fb,
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
    Drm(DrmError),
}

/// Attach a framebuffer for a [`Dmabuf`]
///
/// This tries to import the [`Dmabuf`] using gbm and attach
/// a [`framebuffer::Handle`] for the imported [`BufferObject`]
pub fn framebuffer_from_dmabuf<A: AsFd + 'static>(
    drm: &DrmDeviceFd,
    gbm: &gbm::Device<A>,
    dmabuf: &Dmabuf,
) -> Result<GbmFramebuffer, Error> {
    let bo: BufferObject<()> = dmabuf
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
    )
    .map_err(Error::Drm)
    .map(|fb| GbmFramebuffer {
        _bo: Some(bo),
        fb,
        drm: drm.clone(),
    })
}

/// Possible errors for attaching a [`framebuffer::Handle`] with [`framebuffer_from_bo`]
#[derive(Debug, Error)]
#[error("failed to add a framebuffer")]
pub struct DrmError {
    /// Error message associated to the access error
    pub errmsg: &'static str,
    /// Device on which the error was generated
    pub dev: Option<PathBuf>,
    /// Underlying device error
    pub source: drm::SystemError,
}

/// Attach a [`framebuffer::Handle`] to an [`BufferObject`]
pub fn framebuffer_from_bo<T>(drm: &DrmDeviceFd, bo: &BufferObject<T>) -> Result<GbmFramebuffer, DrmError> {
    framebuffer_from_bo_internal(
        drm,
        BufferObjectInternal {
            bo,
            offsets: None,
            pitches: None,
        },
    )
    .map(|fb| GbmFramebuffer {
        _bo: None,
        fb,
        drm: drm.clone(),
    })
}

struct BufferObjectInternal<'a, T: 'static> {
    bo: &'a BufferObject<T>,
    pitches: Option<[u32; 4]>,
    offsets: Option<[u32; 4]>,
}

impl<'a, T: 'static> std::ops::Deref for BufferObjectInternal<'a, T> {
    type Target = BufferObject<T>;

    fn deref(&self) -> &Self::Target {
        self.bo
    }
}

impl<'a, T: 'static> PlanarBuffer for BufferObjectInternal<'a, T> {
    fn size(&self) -> (u32, u32) {
        PlanarBuffer::size(self.bo)
    }

    fn format(&self) -> drm_fourcc::DrmFourcc {
        PlanarBuffer::format(self.bo)
    }

    fn pitches(&self) -> [u32; 4] {
        self.pitches.unwrap_or_else(|| PlanarBuffer::pitches(self.bo))
    }

    fn handles(&self) -> [Option<drm::buffer::Handle>; 4] {
        PlanarBuffer::handles(self.bo)
    }

    fn offsets(&self) -> [u32; 4] {
        self.offsets.unwrap_or_else(|| PlanarBuffer::offsets(self.bo))
    }
}

fn framebuffer_from_bo_internal<D, T>(
    drm: &D,
    bo: BufferObjectInternal<'_, T>,
) -> Result<framebuffer::Handle, DrmError>
where
    D: drm::control::Device + DevPath,
{
    let modifier = match bo.modifier().unwrap() {
        DrmModifier::Invalid => None,
        x => Some(x),
    };

    let fb = match if modifier.is_some() {
        let num = bo.plane_count().unwrap();
        let modifiers = [
            modifier,
            if num > 1 { modifier } else { None },
            if num > 2 { modifier } else { None },
            if num > 3 { modifier } else { None },
        ];
        drm.add_planar_framebuffer(&bo, &modifiers, drm_ffi::DRM_MODE_FB_MODIFIERS)
    } else {
        drm.add_planar_framebuffer(&bo, &[None, None, None, None], 0)
    } {
        Ok(fb) => fb,
        Err(source) => {
            // We only support this as a fallback of last resort like xf86-video-modesetting does.
            if bo.plane_count().unwrap() > 1 {
                return Err(DrmError {
                    errmsg: "Failed to add framebuffer",
                    dev: drm.dev_path(),
                    source,
                });
            }

            let fourcc = bo.format();
            let (depth, bpp) = get_depth(fourcc)
                .and_then(|d| get_bpp(fourcc).map(|b| (d, b)))
                .ok_or_else(|| DrmError {
                    errmsg: "Unknown format for legacy framebuffer",
                    dev: drm.dev_path(),
                    source,
                })?;

            drm.add_framebuffer(&*bo, depth as u32, bpp as u32)
                .map_err(|source| DrmError {
                    errmsg: "Failed to add framebuffer",
                    dev: drm.dev_path(),
                    source,
                })?
        }
    };
    Ok(fb)
}
