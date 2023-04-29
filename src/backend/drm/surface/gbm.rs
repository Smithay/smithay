use std::collections::HashSet;
use std::sync::Arc;

use drm::control::{connector, crtc, plane, Mode};
use gbm::BufferObject;

use crate::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use crate::backend::allocator::format::get_opaque;
use crate::backend::allocator::gbm::GbmConvertError;
use crate::backend::allocator::{Allocator, Format, Fourcc, Modifier, Slot, Swapchain};
use crate::backend::drm::gbm::{framebuffer_from_bo, GbmFramebuffer};
use crate::backend::drm::{DrmError, DrmSurface};
use crate::backend::SwapBuffersError;
use crate::utils::{Physical, Point, Rectangle, Transform};

use tracing::{debug, error, info_span, instrument, trace, warn};

use super::{PlaneConfig, PlaneDamageClips, PlaneState};

/// Simplified abstraction of a swapchain for gbm-buffers displayed on a [`DrmSurface`].
#[derive(Debug)]
pub struct GbmBufferedSurface<A: Allocator<Buffer = BufferObject<()>> + 'static, U> {
    current_fb: Slot<BufferObject<()>>,
    pending_fb: Option<(Slot<BufferObject<()>>, U)>,
    #[allow(clippy::type_complexity)]
    queued_fb: Option<(Slot<BufferObject<()>>, Option<Vec<Rectangle<i32, Physical>>>, U)>,
    next_fb: Option<Slot<BufferObject<()>>>,
    swapchain: Swapchain<A>,
    drm: Arc<DrmSurface>,
    span: tracing::Span,
}

impl<A, U> GbmBufferedSurface<A, U>
where
    A: Allocator<Buffer = BufferObject<()>>,
    A::Error: std::error::Error + Send + Sync,
{
    /// Create a new `GbmBufferedSurface` from a given compatible combination
    /// of a surface, an allocator and renderer formats.
    ///
    /// The provided color_formats are tested in order until a working configuration is found.
    ///
    /// To successfully call this function, you need to have a renderer,
    /// which can render into a Dmabuf, and a gbm allocator that can produce
    /// buffers of a supported format for rendering.
    pub fn new(
        drm: DrmSurface,
        mut allocator: A,
        color_formats: &[Fourcc],
        renderer_formats: HashSet<Format>,
    ) -> Result<GbmBufferedSurface<A, U>, Error<A::Error>> {
        let span = info_span!(parent: drm.span(), "drm_gbm");
        let _guard = span.enter();

        let mut error = None;
        let drm = Arc::new(drm);

        for format in color_formats {
            debug!("Testing color format: {}", format);
            match Self::new_internal(drm.clone(), allocator, renderer_formats.clone(), *format) {
                Ok((current_fb, swapchain)) => {
                    drop(_guard);
                    return Ok(GbmBufferedSurface {
                        current_fb,
                        pending_fb: None,
                        queued_fb: None,
                        next_fb: None,
                        swapchain,
                        drm,
                        span,
                    });
                }
                Err((alloc, err)) => {
                    warn!("Preferred format {} not available: {:?}", format, err);
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
    ) -> Result<(Slot<BufferObject<()>>, Swapchain<A>), (A, Error<A::Error>)> {
        // select a format
        let mut plane_formats = match drm.supported_formats(drm.plane()) {
            Ok(formats) => formats.iter().cloned().collect::<HashSet<_>>(),
            Err(err) => return Err((allocator, err.into())),
        };

        let opaque_code = get_opaque(code).unwrap_or(code);
        if !plane_formats
            .iter()
            .any(|fmt| fmt.code == code || fmt.code == opaque_code)
        {
            return Err((allocator, Error::NoSupportedPlaneFormat));
        }
        plane_formats.retain(|fmt| fmt.code == code || fmt.code == opaque_code);
        renderer_formats.retain(|fmt| fmt.code == code);

        let plane_modifiers = plane_formats
            .iter()
            .map(|fmt| fmt.modifier)
            .collect::<HashSet<_>>();
        let renderer_modifiers = renderer_formats
            .iter()
            .map(|fmt| fmt.modifier)
            .collect::<HashSet<_>>();

        trace!("Plane formats: {:?}", plane_formats);
        trace!("Renderer formats: {:?}", renderer_formats);
        debug!(
            "Remaining intersected modifiers: {:?}",
            plane_modifiers
                .intersection(&renderer_modifiers)
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
                plane_modifiers
                    .intersection(&renderer_modifiers)
                    .cloned()
                    .map(|modifier| Format { code, modifier })
                    .collect::<Vec<_>>()
            }
        };
        debug!("Testing Formats: {:?}", formats);

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

        let fb = match framebuffer_from_bo(drm.device_fd(), &buffer, true) {
            Ok(fb) => fb,
            Err(err) => return Err((swapchain.allocator, Error::DrmError(err.into()))),
        };
        match buffer.export() {
            Ok(dmabuf) => dmabuf,
            Err(err) => return Err((swapchain.allocator, err.into())),
        };
        buffer.userdata().insert_if_missing(|| fb);

        let handle = buffer.userdata().get::<GbmFramebuffer>().unwrap();

        let plane_state = PlaneState {
            handle: drm.plane(),
            config: Some(PlaneConfig {
                src: Rectangle::from_loc_and_size(
                    Point::default(),
                    (mode.size().0 as i32, mode.size().1 as i32),
                )
                .to_f64(),
                dst: Rectangle::from_loc_and_size(
                    Point::default(),
                    (mode.size().0 as i32, mode.size().1 as i32),
                ),
                transform: Transform::Normal,
                damage_clips: None,
                fb: *handle.as_ref(),
            }),
        };

        match drm.test_state([plane_state], true) {
            Ok(_) => {
                debug!("Choosen format: {:?}", format);
                Ok((buffer, swapchain))
            }
            Err(err) => {
                warn!(
                    "Mode-setting failed with automatically selected buffer format {:?}: {}",
                    format, err
                );
                Err((swapchain.allocator, err.into()))
            }
        }
    }

    /// Retrieves the next buffer to be rendered into and it's age.
    ///
    /// *Note*: This function can be called multiple times and
    /// will return the same buffer until it is queued (see [`GbmBufferedSurface::queue_buffer`]).
    #[instrument(level = "trace", skip_all, parent = &self.span, err)]
    pub fn next_buffer(&mut self) -> Result<(Dmabuf, u8), Error<A::Error>> {
        if self.next_fb.is_none() {
            let slot = self
                .swapchain
                .acquire()
                .map_err(Error::GbmError)?
                .ok_or(Error::NoFreeSlotsError)?;

            let maybe_buffer = slot.userdata().get::<GbmFramebuffer>();
            if maybe_buffer.is_none() {
                let fb = framebuffer_from_bo(self.drm.device_fd(), &slot, true)
                    .map_err(|err| Error::DrmError(err.into()))?;
                slot.userdata().insert_if_missing(|| fb);
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
    pub fn queue_buffer(
        &mut self,
        damage: Option<Vec<Rectangle<i32, Physical>>>,
        user_data: U,
    ) -> Result<(), Error<A::Error>> {
        self.queued_fb = self.next_fb.take().map(|fb| {
            self.swapchain.submitted(&fb);
            (fb, damage, user_data)
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
        let (slot, damage, user_data) = self.queued_fb.take().unwrap();
        let handle = slot.userdata().get::<GbmFramebuffer>().unwrap();
        let mode = self.drm.pending_mode();
        let src =
            Rectangle::from_loc_and_size(Point::default(), (mode.size().0 as i32, mode.size().1 as i32))
                .to_f64();
        let dst =
            Rectangle::from_loc_and_size(Point::default(), (mode.size().0 as i32, mode.size().1 as i32));

        let damage_clips = damage.and_then(|damage| {
            PlaneDamageClips::from_damage(self.drm.device_fd(), src, dst, damage)
                .ok()
                .flatten()
        });

        let plane_state = PlaneState {
            handle: self.plane(),
            config: Some(PlaneConfig {
                src,
                dst,
                transform: Transform::Normal,
                damage_clips: damage_clips.as_ref().map(|d| d.blob()),
                fb: *handle.as_ref(),
            }),
        };

        let flip = if self.drm.commit_pending() {
            self.drm.commit([plane_state], true)
        } else {
            self.drm.page_flip([plane_state], true)
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
