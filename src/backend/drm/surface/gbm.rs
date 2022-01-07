use std::collections::HashSet;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;

use drm::buffer::PlanarBuffer;
use drm::control::{connector, crtc, framebuffer, plane, Device, Mode};
use gbm::{BufferObject, Device as GbmDevice};

use crate::backend::allocator::{
    dmabuf::{AsDmabuf, Dmabuf},
    gbm::GbmConvertError,
    Format, Fourcc, Modifier, Slot, Swapchain,
};
use crate::backend::drm::{device::DevPath, surface::DrmSurfaceInternal, DrmError, DrmSurface};
use crate::backend::SwapBuffersError;

use slog::{debug, error, o, trace, warn};

/// Simplified abstraction of a swapchain for gbm-buffers displayed on a [`DrmSurface`].
#[derive(Debug)]
pub struct GbmBufferedSurface<D: AsRawFd + 'static> {
    current_fb: Slot<BufferObject<()>>,
    pending_fb: Option<Slot<BufferObject<()>>>,
    queued_fb: Option<Slot<BufferObject<()>>>,
    next_fb: Option<Slot<BufferObject<()>>>,
    swapchain: Swapchain<GbmDevice<D>, BufferObject<()>>,
    drm: Arc<DrmSurface<D>>,
}

impl<D> GbmBufferedSurface<D>
where
    D: AsRawFd + 'static,
{
    /// Create a new `GbmBufferedSurface` from a given compatible combination
    /// of a surface, an allocator and renderer formats.
    ///
    /// To successfully call this function, you need to have a renderer,
    /// which can render into a Dmabuf, and a gbm allocator that can produce
    /// buffers of a supported format for rendering.
    #[allow(clippy::type_complexity)]
    pub fn new<L>(
        drm: DrmSurface<D>,
        allocator: GbmDevice<D>,
        mut renderer_formats: HashSet<Format>,
        log: L,
    ) -> Result<GbmBufferedSurface<D>, Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        // we cannot simply pick the first supported format of the intersection of *all* formats, because:
        // - we do not want something like Abgr4444, which looses color information
        // - some formats might perform terribly
        // - we might need some work-arounds, if one supports modifiers, but the other does not
        //
        // So lets just pick `ARGB8888` for now, it is widely supported.
        // Once we have proper color management and possibly HDR support,
        // we need to have a more sophisticated picker.
        // (Or maybe just pick ARGB2101010, if available, we will see.)
        let mut code = Fourcc::Argb8888;
        let logger = crate::slog_or_fallback(log).new(o!("backend" => "drm_render"));

        // select a format
        let mut plane_formats = drm
            .supported_formats(drm.plane())?
            .iter()
            .cloned()
            .collect::<HashSet<_>>();

        // try non alpha variant if not available
        if !plane_formats.iter().any(|fmt| fmt.code == code) {
            code = Fourcc::Xrgb8888;
        }
        plane_formats.retain(|fmt| fmt.code == code);
        renderer_formats.retain(|fmt| fmt.code == code);

        trace!(logger, "Plane formats: {:?}", plane_formats);
        trace!(logger, "Renderer formats: {:?}", renderer_formats);
        debug!(
            logger,
            "Remaining intersected formats: {:?}",
            plane_formats
                .intersection(&renderer_formats)
                .collect::<HashSet<_>>()
        );

        if plane_formats.is_empty() {
            return Err(Error::NoSupportedPlaneFormat);
        } else if renderer_formats.is_empty() {
            return Err(Error::NoSupportedRendererFormat);
        }

        let formats = {
            // Special case: if a format supports explicit LINEAR (but no implicit Modifiers)
            // and the other doesn't support any modifier, force Implicit.
            // This should at least result in a working pipeline possibly with a linear buffer,
            // but we cannot be sure.
            if (plane_formats.len() == 1
                && plane_formats.iter().next().unwrap().modifier == Modifier::Invalid
                && renderer_formats.iter().all(|x| x.modifier != Modifier::Invalid)
                && renderer_formats.iter().any(|x| x.modifier == Modifier::Linear))
                || (renderer_formats.len() == 1
                    && renderer_formats.iter().next().unwrap().modifier == Modifier::Invalid
                    && plane_formats.iter().all(|x| x.modifier != Modifier::Invalid)
                    && plane_formats.iter().any(|x| x.modifier == Modifier::Linear))
            {
                vec![Format {
                    code,
                    modifier: Modifier::Invalid,
                }]
            } else {
                plane_formats
                    .intersection(&renderer_formats)
                    .cloned()
                    .collect::<Vec<_>>()
            }
        };
        debug!(logger, "Testing Formats: {:?}", formats);

        let drm = Arc::new(drm);
        let modifiers = formats.iter().map(|x| x.modifier).collect::<Vec<_>>();

        let mode = drm.pending_mode();

        let mut swapchain: Swapchain<GbmDevice<D>, BufferObject<()>> = Swapchain::new(
            allocator,
            mode.size().0 as u32,
            mode.size().1 as u32,
            code,
            modifiers,
        );

        // Test format
        let buffer = swapchain.acquire()?.unwrap();
        let format = Format {
            code,
            modifier: buffer.modifier().unwrap(), // no guarantee
                                                  // that this is stable across allocations, but
                                                  // we want to print that here for debugging proposes.
                                                  // It has no further use.
        };

        let fb = attach_framebuffer(&drm, &*buffer)?;
        let dmabuf = buffer.export()?;
        let handle = fb.fb;
        buffer.userdata().insert_if_missing(|| dmabuf);
        buffer.userdata().insert_if_missing(|| fb);

        match drm.test_buffer(handle, &mode, true) {
            Ok(_) => {
                debug!(logger, "Choosen format: {:?}", format);
                Ok(GbmBufferedSurface {
                    current_fb: buffer,
                    pending_fb: None,
                    queued_fb: None,
                    next_fb: None,
                    swapchain,
                    drm,
                })
            }
            Err(err) => {
                warn!(
                    logger,
                    "Mode-setting failed with automatically selected buffer format {:?}: {}", format, err
                );
                Err(err).map_err(Into::into)
            }
        }
    }

    /// Retrieves the next buffer to be rendered into and it's age.
    ///
    /// *Note*: This function can be called multiple times and
    /// will return the same buffer until it is queued (see [`GbmBufferedSurface::queue_buffer`]).
    pub fn next_buffer(&mut self) -> Result<(Dmabuf, u8), Error> {
        if self.next_fb.is_none() {
            let slot = self.swapchain.acquire()?.ok_or(Error::NoFreeSlotsError)?;

            let maybe_buffer = slot.userdata().get::<Dmabuf>().cloned();
            if maybe_buffer.is_none() {
                let dmabuf = slot.export().map_err(Error::AsDmabufError)?;
                let fb_handle = attach_framebuffer(&self.drm, &*slot)?;

                let userdata = slot.userdata();
                userdata.insert_if_missing(|| dmabuf);
                userdata.insert_if_missing(|| fb_handle);
            }

            self.next_fb = Some(slot);
        }

        let slot = self.next_fb.as_ref().unwrap();
        Ok((slot.userdata().get::<Dmabuf>().unwrap().clone(), slot.age()))
    }

    /// Queues the current buffer for rendering.
    ///
    /// *Note*: This function needs to be followed up with [`GbmBufferedSurface::frame_submitted`]
    /// when a vblank event is received, that denotes successful scanout of the buffer.
    /// Otherwise the underlying swapchain will eventually run out of buffers.
    pub fn queue_buffer(&mut self) -> Result<(), Error> {
        self.queued_fb = self.next_fb.take();
        if self.pending_fb.is_none() && self.queued_fb.is_some() {
            self.submit()?;
        }
        Ok(())
    }

    /// Marks the current frame as submitted.
    ///
    /// *Note*: Needs to be called, after the vblank event of the matching [`DrmDevice`](super::super::DrmDevice)
    /// was received after calling [`GbmBufferedSurface::queue_buffer`] on this surface.
    /// Otherwise the underlying swapchain will run out of buffers eventually.
    pub fn frame_submitted(&mut self) -> Result<(), Error> {
        if let Some(mut pending) = self.pending_fb.take() {
            std::mem::swap(&mut pending, &mut self.current_fb);
            self.swapchain.submitted(pending);
            if self.queued_fb.is_some() {
                self.submit()?;
            }
        }

        Ok(())
    }

    fn submit(&mut self) -> Result<(), Error> {
        // yes it does not look like it, but both of these lines should be safe in all cases.
        let slot = self.queued_fb.take().unwrap();
        let fb = slot.userdata().get::<FbHandle<D>>().unwrap().fb;

        let flip = if self.drm.commit_pending() {
            self.drm.commit([(fb, self.drm.plane())].iter(), true)
        } else {
            self.drm.page_flip([(fb, self.drm.plane())].iter(), true)
        };
        if flip.is_ok() {
            self.pending_fb = Some(slot);
        }
        flip.map_err(Error::DrmError)
    }

    /// Reset the underlying buffers
    pub fn reset_buffers(&mut self) {
        self.swapchain.reset_buffers()
    }

    /// Returns the underlying [`crtc`](drm::control::crtc) of this surface
    pub fn crtc(&self) -> crtc::Handle {
        self.drm.crtc()
    }

    /// Returns the underlying [`plane`](drm::control::plane) of this surface
    pub fn plane(&self) -> plane::Handle {
        self.drm.plane()
    }

    /// Currently used [`connector`](drm::control::connector)s of this `Surface`
    pub fn current_connectors(&self) -> impl IntoIterator<Item = connector::Handle> {
        self.drm.current_connectors()
    }

    /// Returns the pending [`connector`](drm::control::connector)s
    /// used for the next frame queued via [`queue_buffer`](GbmBufferedSurface::queue_buffer).
    pub fn pending_connectors(&self) -> impl IntoIterator<Item = connector::Handle> {
        self.drm.pending_connectors()
    }

    /// Tries to add a new [`connector`](drm::control::connector)
    /// to be used after the next commit.
    ///
    /// **Warning**: You need to make sure, that the connector is not used with another surface
    /// or was properly removed via `remove_connector` + `commit` before adding it to another surface.
    /// Behavior if failing to do so is undefined, but might result in rendering errors or the connector
    /// getting removed from the other surface without updating it's internal state.
    ///
    /// Fails if the `connector` is not compatible with the underlying [`crtc`](drm::control::crtc)
    /// (e.g. no suitable [`encoder`](drm::control::encoder) may be found)
    /// or is not compatible with the currently pending
    /// [`Mode`](drm::control::Mode).
    pub fn add_connector(&self, connector: connector::Handle) -> Result<(), Error> {
        self.drm.add_connector(connector).map_err(Error::DrmError)
    }

    /// Tries to mark a [`connector`](drm::control::connector)
    /// for removal on the next commit.    
    pub fn remove_connector(&self, connector: connector::Handle) -> Result<(), Error> {
        self.drm.remove_connector(connector).map_err(Error::DrmError)
    }

    /// Tries to replace the current connector set with the newly provided one on the next commit.
    ///
    /// Fails if one new `connector` is not compatible with the underlying [`crtc`](drm::control::crtc)
    /// (e.g. no suitable [`encoder`](drm::control::encoder) may be found)
    /// or is not compatible with the currently pending
    /// [`Mode`](drm::control::Mode).    
    pub fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Error> {
        self.drm.set_connectors(connectors).map_err(Error::DrmError)
    }

    /// Returns the currently active [`Mode`](drm::control::Mode)
    /// of the underlying [`crtc`](drm::control::crtc)    
    pub fn current_mode(&self) -> Mode {
        self.drm.current_mode()
    }

    /// Returns the currently pending [`Mode`](drm::control::Mode)
    /// to be used after the next commit.    
    pub fn pending_mode(&self) -> Mode {
        self.drm.pending_mode()
    }

    /// Tries to set a new [`Mode`](drm::control::Mode)
    /// to be used after the next commit.
    ///
    /// Fails if the mode is not compatible with the underlying
    /// [`crtc`](drm::control::crtc) or any of the
    /// pending [`connector`](drm::control::connector)s.
    pub fn use_mode(&mut self, mode: Mode) -> Result<(), Error> {
        self.drm.use_mode(mode).map_err(Error::DrmError)?;
        let (w, h) = mode.size();
        self.swapchain.resize(w as _, h as _);
        Ok(())
    }
}

#[derive(Debug)]
struct FbHandle<D: AsRawFd + 'static> {
    drm: Arc<DrmSurface<D>>,
    fb: framebuffer::Handle,
}

impl<A: AsRawFd + 'static> Drop for FbHandle<A> {
    fn drop(&mut self) {
        let _ = self.drm.destroy_framebuffer(self.fb);
    }
}

fn attach_framebuffer<A>(drm: &Arc<DrmSurface<A>>, bo: &BufferObject<()>) -> Result<FbHandle<A>, Error>
where
    A: AsRawFd + 'static,
{
    let modifier = match bo.modifier().unwrap() {
        Modifier::Invalid => None,
        x => Some(x),
    };

    let logger = match &*(*drm).internal {
        DrmSurfaceInternal::Atomic(surf) => surf.logger.clone(),
        DrmSurfaceInternal::Legacy(surf) => surf.logger.clone(),
    };

    let fb = match if modifier.is_some() {
        let num = bo.plane_count().unwrap();
        let modifiers = [
            modifier,
            if num > 1 { modifier } else { None },
            if num > 2 { modifier } else { None },
            if num > 3 { modifier } else { None },
        ];
        drm.add_planar_framebuffer(bo, &modifiers, drm_ffi::DRM_MODE_FB_MODIFIERS)
    } else {
        drm.add_planar_framebuffer(bo, &[None, None, None, None], 0)
    } {
        Ok(fb) => fb,
        Err(source) => {
            // We only support this as a fallback of last resort for ARGB8888 visuals,
            // like xf86-video-modesetting does.
            if drm::buffer::Buffer::format(bo) != Fourcc::Argb8888 || bo.handles()[1].is_some() {
                return Err(Error::DrmError(DrmError::Access {
                    errmsg: "Failed to add framebuffer",
                    dev: drm.dev_path(),
                    source,
                }));
            }
            debug!(logger, "Failed to add framebuffer, trying legacy method");
            drm.add_framebuffer(bo, 32, 32)
                .map_err(|source| DrmError::Access {
                    errmsg: "Failed to add framebuffer",
                    dev: drm.dev_path(),
                    source,
                })?
        }
    };
    Ok(FbHandle { drm: drm.clone(), fb })
}

/// Errors thrown by a [`GbmBufferedSurface`]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No supported pixel format for the given plane could be determined
    #[error("No supported plane buffer format found")]
    NoSupportedPlaneFormat,
    /// No supported pixel format for the given renderer could be determined
    #[error("No supported renderer buffer format found")]
    NoSupportedRendererFormat,
    /// The supported pixel formats of the renderer and plane are incompatible
    #[error("Supported plane and renderer buffer formats are incompatible")]
    FormatsNotCompatible,
    /// The swapchain is exhausted, you need to call `frame_submitted`
    #[error("Failed to allocate a new buffer")]
    NoFreeSlotsError,
    /// Failed to renderer using the given renderer
    #[error("Failed to render test frame")]
    InitialRenderingError,
    /// Error accessing the drm device
    #[error("The underlying drm surface encounted an error: {0}")]
    DrmError(#[from] DrmError),
    /// Error importing the rendered buffer to libgbm for scan-out
    #[error("The underlying gbm device encounted an error: {0}")]
    GbmError(#[from] std::io::Error),
    /// Error exporting as Dmabuf
    #[error("The allocated buffer could not be exported as a dmabuf: {0}")]
    AsDmabufError(#[from] GbmConvertError),
}

impl From<Error> for SwapBuffersError {
    fn from(err: Error) -> SwapBuffersError {
        match err {
            x @ Error::NoSupportedPlaneFormat
            | x @ Error::NoSupportedRendererFormat
            | x @ Error::FormatsNotCompatible
            | x @ Error::InitialRenderingError => SwapBuffersError::ContextLost(Box::new(x)),
            x @ Error::NoFreeSlotsError => SwapBuffersError::TemporaryFailure(Box::new(x)),
            Error::DrmError(err) => err.into(),
            Error::GbmError(err) => SwapBuffersError::ContextLost(Box::new(err)),
            Error::AsDmabufError(err) => SwapBuffersError::ContextLost(Box::new(err)),
        }
    }
}
