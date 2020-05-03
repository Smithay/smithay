use super::super::{Device, RawDevice, RawSurface, Surface};
use super::Error;

use drm::control::{connector, crtc, framebuffer, Device as ControlDevice, Mode};
use failure::ResultExt;
use gbm::{self, BufferObject, BufferObjectFlags, Format as GbmFormat, SurfaceBufferHandle};
use image::{ImageBuffer, Rgba};

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::backend::graphics::CursorBackend;

pub(super) struct GbmSurfaceInternal<D: RawDevice + 'static> {
    pub(super) dev: Rc<RefCell<gbm::Device<D>>>,
    pub(super) surface: RefCell<gbm::Surface<framebuffer::Handle>>,
    pub(super) crtc: <D as Device>::Surface,
    pub(super) cursor: Cell<(BufferObject<()>, (u32, u32))>,
    pub(super) current_frame_buffer: Cell<Option<framebuffer::Handle>>,
    pub(super) front_buffer: Cell<Option<SurfaceBufferHandle<framebuffer::Handle>>>,
    pub(super) next_buffer: Cell<Option<SurfaceBufferHandle<framebuffer::Handle>>>,
    pub(super) recreated: Cell<bool>,
    pub(super) logger: ::slog::Logger,
}

impl<D: RawDevice + 'static> GbmSurfaceInternal<D> {
    pub(super) fn unlock_buffer(&self) {
        // after the page swap is finished we need to release the rendered buffer.
        // this is called from the PageFlipHandler
        trace!(self.logger, "Releasing old front buffer");
        self.front_buffer.set(self.next_buffer.replace(None));
        // drop and release the old buffer
    }

    pub unsafe fn page_flip(&self) -> Result<(), Error<<<D as Device>::Surface as Surface>::Error>> {
        let res = {
            let nb = self.next_buffer.take();
            let res = nb.is_some();
            self.next_buffer.set(nb);
            res
        };
        if res {
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
            .borrow()
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
                .add_planar_framebuffer(&*next_bo, &[0; 4], 0)
                .compat()
                .map_err(Error::FramebufferCreationFailed)?;
            next_bo.set_userdata(fb).unwrap();
            fb
        };
        self.next_buffer.set(Some(next_bo));

        if self.recreated.get() {
            debug!(self.logger, "Commiting new state");
            self.crtc.commit(fb).map_err(Error::Underlying)?;
            self.recreated.set(false);
        } else {
            trace!(self.logger, "Queueing Page flip");
            RawSurface::page_flip(&self.crtc, fb).map_err(Error::Underlying)?;
        }

        self.current_frame_buffer.set(Some(fb));

        Ok(())
    }

    pub fn recreate(&self) -> Result<(), Error<<<D as Device>::Surface as Surface>::Error>> {
        let (w, h) = self.pending_mode().size();

        // Recreate the surface and the related resources to match the new
        // resolution.
        debug!(self.logger, "(Re-)Initializing surface (with mode: {}:{})", w, h);
        let surface = self
            .dev
            .borrow_mut()
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
        *self.surface.borrow_mut() = surface;

        self.recreated.set(true);

        Ok(())
    }

    pub fn clear_framebuffers(&self) {
        if let Some(Ok(Some(fb))) = self.next_buffer.take().map(|mut bo| bo.take_userdata()) {
            if let Err(err) = self.crtc.destroy_framebuffer(fb) {
                warn!(
                    self.logger,
                    "Error releasing old back_buffer framebuffer: {:?}", err
                );
            }
        }

        if let Some(Ok(Some(fb))) = self.front_buffer.take().map(|mut bo| bo.take_userdata()) {
            if let Err(err) = self.crtc.destroy_framebuffer(fb) {
                warn!(
                    self.logger,
                    "Error releasing old front_buffer framebuffer: {:?}", err
                );
            }
        }
    }
}

impl<D: RawDevice + 'static> Surface for GbmSurfaceInternal<D> {
    type Connectors = <<D as Device>::Surface as Surface>::Connectors;
    type Error = Error<<<D as Device>::Surface as Surface>::Error>;

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
impl<D: RawDevice + 'static> CursorBackend for GbmSurfaceInternal<D>
where
    <D as RawDevice>::Surface: CursorBackend<CursorFormat = dyn drm::buffer::Buffer>,
    <<D as RawDevice>::Surface as CursorBackend>::Error: ::std::error::Error + Send,
{
    type CursorFormat = ImageBuffer<Rgba<u8>, Vec<u8>>;
    type Error = Error<<<D as Device>::Surface as CursorBackend>::Error>;

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
            .borrow_mut()
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
        self.cursor.set((cursor, hotspot));
        Ok(())
    }
}

impl<D: RawDevice + 'static> Drop for GbmSurfaceInternal<D> {
    fn drop(&mut self) {
        // Drop framebuffers attached to the userdata of the gbm surface buffers.
        // (They don't implement drop, as they need the device)
        self.clear_framebuffers();
    }
}

/// Gbm surface for rendering
pub struct GbmSurface<D: RawDevice + 'static>(pub(super) Rc<GbmSurfaceInternal<D>>);

impl<D: RawDevice + 'static> GbmSurface<D> {
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

impl<D: RawDevice + 'static> Surface for GbmSurface<D> {
    type Connectors = <<D as Device>::Surface as Surface>::Connectors;
    type Error = Error<<<D as Device>::Surface as Surface>::Error>;

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
impl<D: RawDevice + 'static> CursorBackend for GbmSurface<D>
where
    <D as RawDevice>::Surface: CursorBackend<CursorFormat = dyn drm::buffer::Buffer>,
    <<D as RawDevice>::Surface as CursorBackend>::Error: ::std::error::Error + Send,
{
    type CursorFormat = ImageBuffer<Rgba<u8>, Vec<u8>>;
    type Error = Error<<<D as Device>::Surface as CursorBackend>::Error>;

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
}
