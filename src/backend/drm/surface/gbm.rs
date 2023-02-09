use std::collections::HashSet;
use std::sync::Arc;

use drm::control::{connector, crtc, framebuffer, plane, Device, Mode};
use gbm::BufferObject;

use crate::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use crate::backend::allocator::gbm::GbmConvertError;
use crate::backend::allocator::{
    format::{get_bpp, get_depth},
    Allocator, Format, Fourcc, Modifier, Slot, Swapchain,
};
use crate::backend::drm::{surface::DrmSurfaceInternal, DrmError, DrmSurface};
use crate::backend::SwapBuffersError;
use crate::utils::DevPath;

use slog::{debug, error, o, trace, warn};

/// Simplified abstraction of a swapchain for gbm-buffers displayed on a [`DrmSurface`].
#[derive(Debug)]
pub struct GbmBufferedSurface<A: Allocator<Buffer = BufferObject<()>> + 'static, U> {
    current_fb: Slot<BufferObject<()>>,
    pending_fb: Option<(Slot<BufferObject<()>>, U)>,
    queued_fb: Option<(Slot<BufferObject<()>>, U)>,
    next_fb: Option<Slot<BufferObject<()>>>,
    swapchain: Swapchain<A>,
    drm: Arc<DrmSurface>,
}

// we cannot simply pick the first supported format of the intersection of *all* formats, because:
// - we do not want something like Abgr4444, which looses color information, if something better is available
// - some formats might perform terribly
// - we might need some work-arounds, if one supports modifiers, but the other does not
//
// So lets just pick `ARGB8888` or `XRGB8888` for now, they are widely supported.
// Once we have proper color management and possibly HDR support,
// we need to have a more sophisticated picker.
// (Or maybe just select A/XRGB2101010, if available, we will see.)
const SUPPORTED_FORMATS: &[Fourcc] = &[Fourcc::Argb8888, Fourcc::Xrgb8888];

impl<A, U> GbmBufferedSurface<A, U>
where
    A: Allocator<Buffer = BufferObject<()>>,
    A::Error: std::error::Error + Send + Sync,
{
    /// Create a new `GbmBufferedSurface` from a given compatible combination
    /// of a surface, an allocator and renderer formats.
    ///
    /// To successfully call this function, you need to have a renderer,
    /// which can render into a Dmabuf, and a gbm allocator that can produce
    /// buffers of a supported format for rendering.
    pub fn new<L>(
        drm: DrmSurface,
        mut allocator: A,
        renderer_formats: HashSet<Format>,
        log: L,
    ) -> Result<GbmBufferedSurface<A, U>, Error<A::Error>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let mut error = None;
        let drm = Arc::new(drm);

        let log = crate::slog_or_fallback(log).new(o!("backend" => "drm_render"));
        for format in SUPPORTED_FORMATS {
            debug!(log, "Testing color format: {}", format);
            match Self::new_internal(
                drm.clone(),
                allocator,
                renderer_formats.clone(),
                *format,
                log.clone(),
            ) {
                Ok((current_fb, swapchain)) => {
                    return Ok(GbmBufferedSurface {
                        current_fb,
                        pending_fb: None,
                        queued_fb: None,
                        next_fb: None,
                        swapchain,
                        drm,
                    });
                }
                Err((alloc, err)) => {
                    warn!(log, "Preferred format {} not available: {:?}", format, err);
                    allocator = alloc;
                    error = Some(err);
                }
            }
        }
        Err(error.unwrap())
    }

    #[allow(clippy::type_complexity)]
    fn new_internal(
        drm: Arc<DrmSurface>,
        allocator: A,
        mut renderer_formats: HashSet<Format>,
        code: Fourcc,
        logger: slog::Logger,
    ) -> Result<(Slot<BufferObject<()>>, Swapchain<A>), (A, Error<A::Error>)> {
        // select a format
        let mut plane_formats = match drm.supported_formats(drm.plane()) {
            Ok(formats) => formats.iter().cloned().collect::<HashSet<_>>(),
            Err(err) => return Err((allocator, err.into())),
        };

        if !plane_formats.iter().any(|fmt| fmt.code == code) {
            return Err((allocator, Error::NoSupportedPlaneFormat));
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
            return Err((allocator, Error::NoSupportedPlaneFormat));
        } else if renderer_formats.is_empty() {
            return Err((allocator, Error::NoSupportedRendererFormat));
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

        let modifiers = formats.iter().map(|x| x.modifier).collect::<Vec<_>>();
        let mode = drm.pending_mode();

        let mut swapchain: Swapchain<A> = Swapchain::new(
            allocator,
            mode.size().0 as u32,
            mode.size().1 as u32,
            code,
            modifiers,
        );

        // Test format
        let buffer = match swapchain.acquire() {
            Ok(buffer) => buffer.unwrap(),
            Err(err) => return Err((swapchain.allocator, Error::GbmError(err))),
        };
        let format = Format {
            code,
            modifier: buffer.modifier().unwrap(), // no guarantee
                                                  // that this is stable across allocations, but
                                                  // we want to print that here for debugging proposes.
                                                  // It has no further use.
        };

        let fb = match attach_framebuffer(&drm, &buffer) {
            Ok(fb) => fb,
            Err(err) => return Err((swapchain.allocator, err)),
        };
        match buffer.export() {
            Ok(dmabuf) => dmabuf,
            Err(err) => return Err((swapchain.allocator, err.into())),
        };
        let handle = fb.fb;
        buffer.userdata().insert_if_missing(|| fb);

        match drm.test_buffer(handle, &mode, true) {
            Ok(_) => {
                debug!(logger, "Choosen format: {:?}", format);
                Ok((buffer, swapchain))
            }
            Err(err) => {
                warn!(
                    logger,
                    "Mode-setting failed with automatically selected buffer format {:?}: {}", format, err
                );
                Err((swapchain.allocator, err.into()))
            }
        }
    }

    /// Retrieves the next buffer to be rendered into and it's age.
    ///
    /// *Note*: This function can be called multiple times and
    /// will return the same buffer until it is queued (see [`GbmBufferedSurface::queue_buffer`]).
    pub fn next_buffer(&mut self) -> Result<(Dmabuf, u8), Error<A::Error>> {
        if self.next_fb.is_none() {
            let slot = self
                .swapchain
                .acquire()
                .map_err(Error::GbmError)?
                .ok_or(Error::NoFreeSlotsError)?;

            let maybe_buffer = slot.userdata().get::<FbHandle>();
            if maybe_buffer.is_none() {
                let fb_handle = attach_framebuffer(&self.drm, &slot)?;
                slot.userdata().insert_if_missing(|| fb_handle);
            }

            self.next_fb = Some(slot);
        }

        let slot = self.next_fb.as_ref().unwrap();
        Ok((slot.export()?, slot.age()))
    }

    /// Queues the current buffer for rendering.
    ///
    /// *Note*: This function needs to be followed up with [`GbmBufferedSurface::frame_submitted`]
    /// when a vblank event is received, that denotes successful scanout of the buffer.
    /// Otherwise the underlying swapchain will eventually run out of buffers.
    ///
    /// `user_data` can be used to attach some data to a specific buffer and later retrieved with [`GbmBufferedSurface::frame_submitted`]
    pub fn queue_buffer(&mut self, user_data: U) -> Result<(), Error<A::Error>> {
        self.queued_fb = self.next_fb.take().map(|fb| {
            self.swapchain.submitted(&fb);
            (fb, user_data)
        });
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
    ///
    /// Returns the user data that was stored with [`GbmBufferedSurface::queue_buffer`] if a buffer was pending, otherwise
    /// `None` is returned.
    pub fn frame_submitted(&mut self) -> Result<Option<U>, Error<A::Error>> {
        if let Some((mut pending, user_data)) = self.pending_fb.take() {
            std::mem::swap(&mut pending, &mut self.current_fb);
            if self.queued_fb.is_some() {
                self.submit()?;
            }
            Ok(Some(user_data))
        } else {
            Ok(None)
        }
    }

    fn submit(&mut self) -> Result<(), Error<A::Error>> {
        // yes it does not look like it, but both of these lines should be safe in all cases.
        let (slot, user_data) = self.queued_fb.take().unwrap();
        let fb = slot.userdata().get::<FbHandle>().unwrap().fb;

        let flip = if self.drm.commit_pending() {
            self.drm.commit([(fb, self.drm.plane())].iter(), true)
        } else {
            self.drm.page_flip([(fb, self.drm.plane())].iter(), true)
        };
        if flip.is_ok() {
            self.pending_fb = Some((slot, user_data));
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
    pub fn add_connector(&self, connector: connector::Handle) -> Result<(), Error<A::Error>> {
        self.drm.add_connector(connector).map_err(Error::DrmError)
    }

    /// Tries to mark a [`connector`](drm::control::connector)
    /// for removal on the next commit.    
    pub fn remove_connector(&self, connector: connector::Handle) -> Result<(), Error<A::Error>> {
        self.drm.remove_connector(connector).map_err(Error::DrmError)
    }

    /// Tries to replace the current connector set with the newly provided one on the next commit.
    ///
    /// Fails if one new `connector` is not compatible with the underlying [`crtc`](drm::control::crtc)
    /// (e.g. no suitable [`encoder`](drm::control::encoder) may be found)
    /// or is not compatible with the currently pending
    /// [`Mode`](drm::control::Mode).    
    pub fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Error<A::Error>> {
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
    pub fn use_mode(&mut self, mode: Mode) -> Result<(), Error<A::Error>> {
        self.drm.use_mode(mode).map_err(Error::DrmError)?;
        let (w, h) = mode.size();
        self.swapchain.resize(w as _, h as _);
        Ok(())
    }

    /// Returns a reference to the underlying drm surface
    pub fn surface(&self) -> &DrmSurface {
        &self.drm
    }

    /// Get the format of the underlying swapchain
    pub fn format(&self) -> Fourcc {
        self.swapchain.format()
    }
}

#[derive(Debug)]
struct FbHandle {
    drm: Arc<DrmSurface>,
    fb: framebuffer::Handle,
}

impl Drop for FbHandle {
    fn drop(&mut self) {
        let _ = self.drm.destroy_framebuffer(self.fb);
    }
}

fn attach_framebuffer<E>(drm: &Arc<DrmSurface>, bo: &BufferObject<()>) -> Result<FbHandle, Error<E>>
where
    E: std::error::Error + Send + Sync,
{
    let modifier = match bo.modifier().unwrap() {
        Modifier::Invalid => None,
        x => Some(x),
    };

    let logger = match &*drm.internal {
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
            // We only support this as a fallback of last resort like xf86-video-modesetting does.
            if bo.plane_count().unwrap() > 1 {
                return Err(Error::DrmError(DrmError::Access {
                    errmsg: "Failed to add framebuffer",
                    dev: drm.dev_path(),
                    source,
                }));
            }
            debug!(logger, "Failed to add framebuffer, trying legacy method");
            let fourcc = bo.format().unwrap();
            let (depth, bpp) = get_depth(fourcc)
                .and_then(|d| get_bpp(fourcc).map(|b| (d, b)))
                .ok_or_else(|| {
                    Error::DrmError(DrmError::Access {
                        errmsg: "Unknown format for legacy framebuffer",
                        dev: drm.dev_path(),
                        source,
                    })
                })?;
            drm.add_framebuffer(bo, depth as u32, bpp as u32)
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
pub enum Error<E: std::error::Error + Send + Sync + 'static> {
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
    GbmError(#[source] E),
    /// Error exporting as Dmabuf
    #[error("The allocated buffer could not be exported as a dmabuf: {0}")]
    AsDmabufError(#[from] GbmConvertError),
}

impl<E: std::error::Error + Send + Sync + 'static> From<Error<E>> for SwapBuffersError {
    fn from(err: Error<E>) -> SwapBuffersError {
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
