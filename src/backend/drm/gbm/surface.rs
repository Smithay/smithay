use super::super::{RawSurface, Surface};
use super::Error;

use drm::control::{connector, crtc, framebuffer, Mode};
use failure::ResultExt;
use gbm::{self, BufferObject, BufferObjectFlags, Format as GbmFormat};
use image::{ImageBuffer, Rgba};

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use crate::backend::graphics::CursorBackend;

pub struct Buffers {
    pub(super) current_frame_buffer: Option<framebuffer::Handle>,
    pub(super) front_buffer: Option<BufferObject<framebuffer::Handle>>,
    pub(super) next_buffer: Option<BufferObject<framebuffer::Handle>>,
}

pub(super) struct GbmSurfaceInternal<S: RawSurface + 'static> {
    pub(super) dev: Arc<Mutex<gbm::Device<gbm::FdWrapper>>>,
    pub(super) surface: Mutex<gbm::Surface<framebuffer::Handle>>,
    pub(super) crtc: S,
    pub(super) cursor: Mutex<(BufferObject<()>, (u32, u32))>,
    pub(super) buffers: Mutex<Buffers>,
    pub(super) recreated: AtomicBool,
    pub(super) logger: ::slog::Logger,
}

impl<S: RawSurface + 'static> GbmSurfaceInternal<S> {
    pub(super) fn new(
        dev: Arc<Mutex<gbm::Device<gbm::FdWrapper>>>,
        drm_surface: S,
        logger: ::slog::Logger,
    ) -> Result<GbmSurfaceInternal<S>, Error<<S as Surface>::Error>> {
        // initialize the surface
        let (w, h) = drm_surface.pending_mode().size();
        let surface = dev
            .lock()
            .unwrap()
            .create_surface(
                w as u32,
                h as u32,
                GbmFormat::XRGB8888,
                BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING,
            )
            .map_err(Error::SurfaceCreationFailed)?;

        // initialize a buffer for the cursor image
        let cursor = (
            dev.lock()
                .unwrap()
                .create_buffer_object(
                    1,
                    1,
                    GbmFormat::ARGB8888,
                    BufferObjectFlags::CURSOR | BufferObjectFlags::WRITE,
                )
                .map_err(Error::BufferCreationFailed)?,
            (0, 0),
        );

        Ok(GbmSurfaceInternal {
            dev,
            surface: Mutex::new(surface),
            crtc: drm_surface,
            cursor: Mutex::new(cursor),
            buffers: Mutex::new(Buffers {
                current_frame_buffer: None,
                front_buffer: None,
                next_buffer: None,
            }),
            recreated: AtomicBool::new(true),
            logger,
        })
    }

    pub(super) fn unlock_buffer(&self) {
        // after the page swap is finished we need to release the rendered buffer.
        // this is called from the PageFlipHandler
        trace!(self.logger, "Releasing old front buffer");
        let mut buffers = self.buffers.lock().unwrap();
        buffers.front_buffer = buffers.next_buffer.take();
        // drop and release the old buffer
    }

    pub unsafe fn page_flip(&self) -> Result<(), Error<<S as Surface>::Error>> {
        let (result, fb) = {
            let mut buffers = self.buffers.lock().unwrap();
            if buffers.next_buffer.is_some() {
                // We cannot call lock_front_buffer anymore without releasing the previous buffer, which will happen when the page flip is done
                warn!(self.logger, "Tried to swap with an already queued flip");
                return Err(Error::FrontBuffersExhausted);
            }

            // supporting only one buffer would cause a lot of inconvinience and
            // would most likely result in a lot of flickering.
            // neither weston, wlc or wlroots bother with that as well.
            // so we just assume we got at least two buffers to do flipping.
            let mut next_bo = self
                .surface
                .lock()
                .unwrap()
                .lock_front_buffer()
                .map_err(|_| Error::FrontBufferLockFailed)?;

            // create a framebuffer if the front buffer does not have one already
            // (they are reused by gbm)
            let maybe_fb = next_bo
                .userdata()
                .map_err(|_| Error::InvalidInternalState)?
                .cloned();
            let fb = if let Some(info) = maybe_fb {
                info
            } else {
                let fb = self
                    .crtc
                    .add_planar_framebuffer(&next_bo, &[0; 4], 0)
                    .compat()
                    .map_err(Error::FramebufferCreationFailed)?;
                next_bo.set_userdata(fb).unwrap();
                fb
            };
            buffers.next_buffer = Some(next_bo);

            if cfg!(debug_assertions) {
                if let Err(err) = self.crtc.get_framebuffer(fb) {
                    error!(self.logger, "Cached framebuffer invalid: {:?}: {}", fb, err);
                }
            }

            // if we re-created the surface, we need to commit the new changes, as we might trigger a modeset
            (
                if self.recreated.load(Ordering::SeqCst) {
                    debug!(self.logger, "Commiting new state");
                    self.crtc.commit(fb).map_err(Error::Underlying)
                } else {
                    trace!(self.logger, "Queueing Page flip");
                    RawSurface::page_flip(&self.crtc, fb).map_err(Error::Underlying)
                },
                fb,
            )
        };

        // if it was successful, we can clear the re-created state
        match result {
            Ok(_) => {
                self.recreated.store(false, Ordering::SeqCst);
                let mut buffers = self.buffers.lock().unwrap();
                buffers.current_frame_buffer = Some(fb);
                Ok(())
            }
            Err(err) => {
                // if there was an error we need to free the buffer again,
                // otherwise we may never lock again.
                self.unlock_buffer();
                Err(err)
            }
        }
    }

    // this function is called, if we e.g. need to create the surface to match a new mode.
    pub fn recreate(&self) -> Result<(), Error<<S as Surface>::Error>> {
        let (w, h) = self.pending_mode().size();

        // Recreate the surface and the related resources to match the new
        // resolution.
        debug!(self.logger, "(Re-)Initializing surface (with mode: {}:{})", w, h);
        let surface = self
            .dev
            .lock()
            .unwrap()
            .create_surface(
                w as u32,
                h as u32,
                GbmFormat::XRGB8888,
                BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING,
            )
            .map_err(Error::SurfaceCreationFailed)?;

        // Clean up buffers
        self.clear_framebuffers();

        // Drop the old surface after cleanup
        *self.surface.lock().unwrap() = surface;

        self.recreated.store(true, Ordering::SeqCst);

        Ok(())
    }

    // if the underlying drm-device is closed and re-opened framebuffers may get invalided.
    // here we clear them just to be sure, they get recreated on the next page_flip.
    pub fn clear_framebuffers(&self) {
        let mut buffers = self.buffers.lock().unwrap();
        if let Some(Ok(Some(fb))) = buffers.next_buffer.take().map(|mut bo| bo.take_userdata()) {
            if let Err(err) = self.crtc.destroy_framebuffer(fb) {
                warn!(
                    self.logger,
                    "Error releasing old back_buffer framebuffer: {:?}", err
                );
            }
        }

        if let Some(Ok(Some(fb))) = buffers.front_buffer.take().map(|mut bo| bo.take_userdata()) {
            if let Err(err) = self.crtc.destroy_framebuffer(fb) {
                warn!(
                    self.logger,
                    "Error releasing old front_buffer framebuffer: {:?}", err
                );
            }
        }
    }
}

impl<S: RawSurface + 'static> Surface for GbmSurfaceInternal<S> {
    type Connectors = <S as Surface>::Connectors;
    type Error = Error<<S as Surface>::Error>;

    fn crtc(&self) -> crtc::Handle {
        self.crtc.crtc()
    }

    fn current_connectors(&self) -> Self::Connectors {
        self.crtc.current_connectors()
    }

    fn pending_connectors(&self) -> Self::Connectors {
        self.crtc.pending_connectors()
    }

    fn add_connector(&self, connector: connector::Handle) -> Result<(), Self::Error> {
        self.crtc.add_connector(connector).map_err(Error::Underlying)
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<(), Self::Error> {
        self.crtc.remove_connector(connector).map_err(Error::Underlying)
    }

    fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Self::Error> {
        self.crtc.set_connectors(connectors).map_err(Error::Underlying)
    }

    fn current_mode(&self) -> Mode {
        self.crtc.current_mode()
    }

    fn pending_mode(&self) -> Mode {
        self.crtc.pending_mode()
    }

    fn use_mode(&self, mode: Mode) -> Result<(), Self::Error> {
        self.crtc.use_mode(mode).map_err(Error::Underlying)
    }
}

#[cfg(feature = "backend_drm")]
impl<S: RawSurface + 'static> CursorBackend for GbmSurfaceInternal<S>
where
    S: CursorBackend<CursorFormat = dyn drm::buffer::Buffer>,
    <S as CursorBackend>::Error: ::std::error::Error + Send,
{
    type CursorFormat = ImageBuffer<Rgba<u8>, Vec<u8>>;
    type Error = Error<<S as CursorBackend>::Error>;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<(), Self::Error> {
        self.crtc.set_cursor_position(x, y).map_err(Error::Underlying)
    }

    fn set_cursor_representation(
        &self,
        buffer: &ImageBuffer<Rgba<u8>, Vec<u8>>,
        hotspot: (u32, u32),
    ) -> Result<(), Self::Error> {
        let (w, h) = buffer.dimensions();
        debug!(self.logger, "Importing cursor");

        // import the cursor into a buffer we can render
        let mut cursor = self
            .dev
            .lock()
            .unwrap()
            .create_buffer_object(
                w,
                h,
                GbmFormat::ARGB8888,
                BufferObjectFlags::CURSOR | BufferObjectFlags::WRITE,
            )
            .map_err(Error::BufferCreationFailed)?;

        cursor
            .write(&**buffer)
            .map_err(|_| Error::DeviceDestroyed)?
            .map_err(Error::BufferWriteFailed)?;

        trace!(self.logger, "Setting the new imported cursor");

        self.crtc
            .set_cursor_representation(&cursor, hotspot)
            .map_err(Error::Underlying)?;

        // and store it
        *self.cursor.lock().unwrap() = (cursor, hotspot);
        Ok(())
    }

    fn clear_cursor_representation(&self) -> Result<(), Self::Error> {
        *self.cursor.lock().unwrap() = (self.dev.lock()
                .unwrap()
                .create_buffer_object(
                    1,
                    1,
                    GbmFormat::ARGB8888,
                    BufferObjectFlags::CURSOR | BufferObjectFlags::WRITE,
                )
                .map_err(Error::BufferCreationFailed)?,
            (0, 0)
        );
        self.crtc.clear_cursor_representation()
            .map_err(Error::Underlying)
    }
}

impl<S: RawSurface + 'static> Drop for GbmSurfaceInternal<S> {
    fn drop(&mut self) {
        // Drop framebuffers attached to the userdata of the gbm surface buffers.
        // (They don't implement drop, as they need the device)
        self.clear_framebuffers();
    }
}

/// Gbm surface for rendering
pub struct GbmSurface<S: RawSurface + 'static>(pub(super) Arc<GbmSurfaceInternal<S>>);

impl<S: RawSurface + 'static> GbmSurface<S> {
    /// Flips the underlying buffers.
    ///
    /// The surface will report being already flipped until the matching event
    /// was processed either by calling [`Device::process_events`] manually after the flip
    /// (bad idea performance-wise) or by binding the device to an event-loop by using
    /// [`device_bind`](::backend::drm::device_bind).
    ///
    /// *Note*: This might trigger a full modeset on the underlying device,
    /// potentially causing some flickering. In that case this operation is
    /// blocking until the crtc is in the desired state.
    ///
    /// # Safety
    ///
    /// When used in conjunction with an EGL context, this must be called exactly once
    /// after page-flipping the associated context.
    pub unsafe fn page_flip(&self) -> ::std::result::Result<(), <Self as Surface>::Error> {
        self.0.page_flip()
    }

    /// Recreate underlying gbm resources.
    ///
    /// This recreates the gbm surfaces resources, which might be needed after e.g.
    /// calling [`Surface::use_mode`](Surface::use_mode).
    /// You may check if your [`GbmSurface`] needs recreation through
    /// [`needs_recreation`](GbmSurface::needs_recreation).
    pub fn recreate(&self) -> Result<(), <Self as Surface>::Error> {
        self.0.recreate()
    }

    /// Check if underlying gbm resources need to be recreated.
    pub fn needs_recreation(&self) -> bool {
        self.0.crtc.commit_pending()
    }
}

impl<S: RawSurface + 'static> Surface for GbmSurface<S> {
    type Connectors = <S as Surface>::Connectors;
    type Error = Error<<S as Surface>::Error>;

    fn crtc(&self) -> crtc::Handle {
        self.0.crtc()
    }

    fn current_connectors(&self) -> Self::Connectors {
        self.0.current_connectors()
    }

    fn pending_connectors(&self) -> Self::Connectors {
        self.0.pending_connectors()
    }

    fn add_connector(&self, connector: connector::Handle) -> Result<(), Self::Error> {
        self.0.add_connector(connector)
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<(), Self::Error> {
        self.0.remove_connector(connector)
    }

    fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Self::Error> {
        self.0.set_connectors(connectors)
    }

    fn current_mode(&self) -> Mode {
        self.0.current_mode()
    }

    fn pending_mode(&self) -> Mode {
        self.0.pending_mode()
    }

    fn use_mode(&self, mode: Mode) -> Result<(), Self::Error> {
        self.0.use_mode(mode)
    }
}

#[cfg(feature = "backend_drm")]
impl<S: RawSurface + 'static> CursorBackend for GbmSurface<S>
where
    S: CursorBackend<CursorFormat = dyn drm::buffer::Buffer>,
    <S as CursorBackend>::Error: ::std::error::Error + Send,
{
    type CursorFormat = ImageBuffer<Rgba<u8>, Vec<u8>>;
    type Error = Error<<S as CursorBackend>::Error>;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<(), Self::Error> {
        self.0.set_cursor_position(x, y)
    }

    fn set_cursor_representation(
        &self,
        buffer: &ImageBuffer<Rgba<u8>, Vec<u8>>,
        hotspot: (u32, u32),
    ) -> Result<(), Self::Error> {
        self.0.set_cursor_representation(buffer, hotspot)
    }

    fn clear_cursor_representation(&self) -> Result<(), Self::Error> {
        self.0.clear_cursor_representation()
    }
}

#[cfg(test)]
mod test {
    use super::GbmSurface;
    use crate::backend::drm::legacy::LegacyDrmSurface;
    use std::fs::File;

    fn is_send<S: Send>() {}

    #[test]
    fn surface_is_send() {
        is_send::<GbmSurface<LegacyDrmSurface<File>>>();
    }
}
