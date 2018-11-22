use super::super::{Device, RawDevice, RawSurface, Surface};
use super::error::*;

use drm::control::{connector, crtc, framebuffer, Mode, ResourceHandles, ResourceInfo};
use gbm::{self, BufferObject, BufferObjectFlags, Format as GbmFormat, SurfaceBufferHandle};
use image::{ImageBuffer, Rgba};

use std::cell::{Cell, RefCell};
use std::os::unix::io::AsRawFd;
use std::rc::Rc;

#[cfg(feature = "backend_drm_legacy")]
use backend::drm::legacy::LegacyDrmDevice;
use backend::graphics::CursorBackend;
use backend::graphics::SwapBuffersError;

pub(super) struct GbmSurfaceInternal<D: RawDevice + 'static> {
    pub(super) dev: Rc<RefCell<gbm::Device<D>>>,
    pub(super) surface: RefCell<gbm::Surface<framebuffer::Info>>,
    pub(super) crtc: <D as Device>::Surface,
    pub(super) cursor: Cell<(BufferObject<()>, (u32, u32))>,
    pub(super) current_frame_buffer: Cell<Option<framebuffer::Info>>,
    pub(super) front_buffer: Cell<Option<SurfaceBufferHandle<framebuffer::Info>>>,
    pub(super) next_buffer: Cell<Option<SurfaceBufferHandle<framebuffer::Info>>>,
    pub(super) logger: ::slog::Logger,
}

impl<D: RawDevice + 'static> GbmSurfaceInternal<D> {
    pub(super) fn unlock_buffer(&self) {
        // after the page swap is finished we need to release the rendered buffer.
        // this is called from the PageFlipHandler
        if let Some(next_buffer) = self.next_buffer.replace(None) {
            trace!(self.logger, "Releasing old front buffer");
            self.front_buffer.set(Some(next_buffer));
            // drop and release the old buffer
        }
    }

    pub fn page_flip<F>(&self, flip: F) -> ::std::result::Result<(), SwapBuffersError>
    where
        F: FnOnce() -> ::std::result::Result<(), SwapBuffersError>,
    {
        let res = {
            let nb = self.next_buffer.take();
            let res = nb.is_some();
            self.next_buffer.set(nb);
            res
        };
        if res {
            // We cannot call lock_front_buffer anymore without releasing the previous buffer, which will happen when the page flip is done
            warn!(self.logger, "Tried to swap with an already queued flip");
            return Err(SwapBuffersError::AlreadySwapped);
        }

        // flip normally
        flip()?;

        // supporting only one buffer would cause a lot of inconvinience and
        // would most likely result in a lot of flickering.
        // neither weston, wlc or wlroots bother with that as well.
        // so we just assume we got at least two buffers to do flipping.
        let mut next_bo = self
            .surface
            .borrow()
            .lock_front_buffer()
            .expect("Surface only has one front buffer. Not supported by smithay");

        // create a framebuffer if the front buffer does not have one already
        // (they are reused by gbm)
        let maybe_fb = next_bo
            .userdata()
            .map_err(|_| SwapBuffersError::ContextLost)?
            .cloned();
        let fb = if let Some(info) = maybe_fb {
            info
        } else {
            let fb = framebuffer::create(&self.crtc, &*next_bo).map_err(|_| SwapBuffersError::ContextLost)?;
            next_bo.set_userdata(fb).unwrap();
            fb
        };
        self.next_buffer.set(Some(next_bo));

        trace!(self.logger, "Queueing Page flip");
        self.crtc.page_flip(fb.handle())?;

        self.current_frame_buffer.set(Some(fb));

        Ok(())
    }

    pub fn recreate<F>(&self, flip: F) -> Result<()>
    where
        F: FnOnce() -> ::std::result::Result<(), SwapBuffersError>,
    {
        let (w, h) = self.pending_mode().size();

        // Recreate the surface and the related resources to match the new
        // resolution.
        debug!(self.logger, "(Re-)Initializing surface for mode: {}:{}", w, h);
        let surface = self
            .dev
            .borrow_mut()
            .create_surface(
                w as u32,
                h as u32,
                GbmFormat::XRGB8888,
                BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING,
            ).chain_err(|| ErrorKind::SurfaceCreationFailed)?;

        // Clean up next_buffer
        {
            if let Some(mut old_bo) = self.next_buffer.take() {
                if let Ok(Some(fb)) = old_bo.take_userdata() {
                    if let Err(err) = framebuffer::destroy(&self.crtc, fb.handle()) {
                        warn!(
                            self.logger,
                            "Error releasing old back_buffer framebuffer: {:?}", err
                        );
                    }
                }
            }
        }

        flip()?;
        
        // Cleanup front_buffer and init the first screen on the new front_buffer
        // (must be done before calling page_flip for the first time)
        let old_front_bo = self.front_buffer.replace({
            let mut front_bo = surface
                .lock_front_buffer()
                .chain_err(|| ErrorKind::FrontBufferLockFailed)?;

            debug!(self.logger, "FrontBuffer color format: {:?}", front_bo.format());

            // we also need a new framebuffer for the front buffer
            let fb = framebuffer::create(&self.crtc, &*front_bo)
                .chain_err(|| ErrorKind::UnderlyingBackendError)?;

            self.crtc
                .commit(fb.handle())
                .chain_err(|| ErrorKind::UnderlyingBackendError)?;

            self.current_frame_buffer.set(Some(fb));
            front_bo.set_userdata(fb).unwrap();
            Some(front_bo)
        });
        if let Some(Ok(Some(fb))) = old_front_bo.map(|mut bo| bo.take_userdata()) {
            if let Err(err) = framebuffer::destroy(&self.crtc, fb.handle()) {
                warn!(
                    self.logger,
                    "Error releasing old front_buffer framebuffer: {:?}", err
                );
            }
        }

        // Drop the old surface after cleanup
        *self.surface.borrow_mut() = surface;

        Ok(())
    }
}

impl<D: RawDevice + 'static> Surface for GbmSurfaceInternal<D> {
    type Connectors = <<D as Device>::Surface as Surface>::Connectors;
    type Error = Error;

    fn crtc(&self) -> crtc::Handle {
        self.crtc.crtc()
    }

    fn current_connectors(&self) -> Self::Connectors {
        self.crtc.current_connectors()
    }

    fn pending_connectors(&self) -> Self::Connectors {
        self.crtc.pending_connectors()
    }

    fn add_connector(&self, connector: connector::Handle) -> Result<()> {
        self.crtc
            .add_connector(connector)
            .chain_err(|| ErrorKind::UnderlyingBackendError)
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<()> {
        self.crtc
            .remove_connector(connector)
            .chain_err(|| ErrorKind::UnderlyingBackendError)
    }

    fn current_mode(&self) -> Mode {
        self.crtc.current_mode()
    }

    fn pending_mode(&self) -> Mode {
        self.crtc.pending_mode()
    }

    fn use_mode(&self, mode: Mode) -> Result<()> {
        self.crtc
            .use_mode(mode)
            .chain_err(|| ErrorKind::UnderlyingBackendError)
    }
}

// FIXME:
//
// Option 1: When there is GAT support, impl `GraphicsBackend` for `LegacyDrmBackend`
//           using a new generic `B: Buffer` and use this:
/*
impl<'a, D: RawDevice + 'static> CursorBackend<'a> for GbmSurfaceInternal<D>
where
    <D as RawDevice>::Surface: CursorBackend<'a>,
    <<D as RawDevice>::Surface as CursorBackend<'a>>::CursorFormat: Buffer,
    <<D as RawDevice>::Surface as CursorBackend<'a>>::Error: ::std::error::Error + Send 
{
*/
//
// Option 2: When equality checks in where clauses are supported, we could at least do this:
/*
impl<'a, D: RawDevice + 'static> GraphicsBackend<'a> for GbmSurfaceInternal<D>
where
    <D as RawDevice>::Surface: CursorBackend<'a>,
    <<D as RawDevice>::Surface as CursorBackend<'a>>::CursorFormat=&'a Buffer,
    <<D as RawDevice>::Surface as CursorBackend<'a>>::Error: ::std::error::Error + Send 
{
*/
// But for now got to do this:

#[cfg(feature = "backend_drm_legacy")]
impl<'a, A: AsRawFd + 'static> CursorBackend<'a> for GbmSurfaceInternal<LegacyDrmDevice<A>> {
    type CursorFormat = &'a ImageBuffer<Rgba<u8>, Vec<u8>>;
    type Error = Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<()> {
        ResultExt::chain_err(self.crtc.set_cursor_position(x, y), || {
            ErrorKind::UnderlyingBackendError
        })
    }

    fn set_cursor_representation<'b>(
        &'b self,
        buffer: &ImageBuffer<Rgba<u8>, Vec<u8>>,
        hotspot: (u32, u32),
    ) -> Result<()>
    where
        'a: 'b,
    {
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
            ).chain_err(|| ErrorKind::BufferCreationFailed)?;

        cursor
            .write(&**buffer)
            .chain_err(|| ErrorKind::BufferWriteFailed)?
            .chain_err(|| ErrorKind::BufferWriteFailed)?;

        trace!(self.logger, "Setting the new imported cursor");

        ResultExt::chain_err(self.crtc.set_cursor_representation(&cursor, hotspot), || {
            ErrorKind::UnderlyingBackendError
        })?;

        // and store it
        self.cursor.set((cursor, hotspot));
        Ok(())
    }
}

impl<D: RawDevice + 'static> Drop for GbmSurfaceInternal<D> {
    fn drop(&mut self) {
        // Drop framebuffers attached to the userdata of the gbm surface buffers.
        // (They don't implement drop, as they need the device)
        if let Ok(Some(fb)) = {
            if let Some(mut next) = self.next_buffer.take() {
                next.take_userdata()
            } else {
                Ok(None)
            }
        } {
            // ignore failure at this point
            let _ = framebuffer::destroy(&self.crtc, fb.handle());
        }

        if let Ok(Some(fb)) = {
            if let Some(mut next) = self.front_buffer.take() {
                next.take_userdata()
            } else {
                Ok(None)
            }
        } {
            // ignore failure at this point
            let _ = framebuffer::destroy(&self.crtc, fb.handle());
        }
    }
}

pub struct GbmSurface<D: RawDevice + 'static>(pub(super) Rc<GbmSurfaceInternal<D>>);

impl<D: RawDevice + 'static> GbmSurface<D> {
    pub fn page_flip<F>(&self, flip: F) -> ::std::result::Result<(), SwapBuffersError>
    where
        F: FnOnce() -> ::std::result::Result<(), SwapBuffersError>,
    {
        self.0.page_flip(flip)
    }

    pub fn recreate<F>(&self, flip: F) -> Result<()>
    where
        F: FnOnce() -> ::std::result::Result<(), SwapBuffersError>,
    {
        self.0.recreate(flip)
    }
}

impl<D: RawDevice + 'static> Surface for GbmSurface<D> {
    type Connectors = <<D as Device>::Surface as Surface>::Connectors;
    type Error = Error;

    fn crtc(&self) -> crtc::Handle {
        self.0.crtc()
    }

    fn current_connectors(&self) -> Self::Connectors {
        self.0.current_connectors()
    }

    fn pending_connectors(&self) -> Self::Connectors {
        self.0.pending_connectors()
    }

    fn add_connector(&self, connector: connector::Handle) -> Result<()> {
        self.0.add_connector(connector)
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<()> {
        self.0.remove_connector(connector)
    }

    fn current_mode(&self) -> Mode {
        self.0.current_mode()
    }

    fn pending_mode(&self) -> Mode {
        self.0.pending_mode()
    }

    fn use_mode(&self, mode: Mode) -> Result<()> {
        self.0.use_mode(mode)
    }
}

#[cfg(feature = "backend_drm_legacy")]
impl<'a, A: AsRawFd + 'static> CursorBackend<'a> for GbmSurface<LegacyDrmDevice<A>> {
    type CursorFormat = &'a ImageBuffer<Rgba<u8>, Vec<u8>>;
    type Error = Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<()> {
        self.0.set_cursor_position(x, y)
    }

    fn set_cursor_representation<'b>(
        &'b self,
        buffer: &ImageBuffer<Rgba<u8>, Vec<u8>>,
        hotspot: (u32, u32),
    ) -> Result<()>
    where
        'a: 'b,
    {
        self.0.set_cursor_representation(buffer, hotspot)
    }
}
