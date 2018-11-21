use super::error::*;
use super::super::{Device, RawDevice, Surface, RawSurface};

use drm::control::{crtc, connector, framebuffer, Mode, ResourceInfo};
use gbm::{self, SurfaceBufferHandle, Format as GbmFormat, BufferObject, BufferObjectFlags};
use image::{ImageBuffer, Rgba};

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::os::unix::io::AsRawFd;

use backend::drm::legacy::{LegacyDrmDevice, LegacyDrmSurface};
use backend::graphics::CursorBackend;
use backend::graphics::SwapBuffersError;

pub struct GbmSurface<D: RawDevice + 'static>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>
{
    pub(in super) dev: Rc<RefCell<gbm::Device<D>>>,
    pub(in super) surface: RefCell<gbm::Surface<framebuffer::Info>>,
    pub(in super) crtc: <D as Device>::Return,
    pub(in super) cursor: Cell<(BufferObject<()>, (u32, u32))>,
    pub(in super) current_frame_buffer: Cell<framebuffer::Info>,
    pub(in super) front_buffer: Cell<SurfaceBufferHandle<framebuffer::Info>>,
    pub(in super) next_buffer: Cell<Option<SurfaceBufferHandle<framebuffer::Info>>>,
    pub(in super) logger: ::slog::Logger,
}

impl<D: RawDevice + 'static> GbmSurface<D>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>
{
    pub(in super) fn unlock_buffer(&self) {
        // after the page swap is finished we need to release the rendered buffer.
        // this is called from the PageFlipHandler
        if let Some(next_buffer) = self.next_buffer.replace(None) {
            trace!(self.logger, "Releasing old front buffer");
            self.front_buffer.set(next_buffer);
            // drop and release the old buffer
        }
    }

    pub fn page_flip<F>(&self, flip: F) -> ::std::result::Result<(), SwapBuffersError>
    where
        F: FnOnce() -> ::std::result::Result<(), SwapBuffersError>
    {
        let res = {
            let nb = self.next_buffer.take();
            let res = nb.is_some();
            self.next_buffer.set(nb);
            res
        };
        if res {
            // We cannot call lock_front_buffer anymore without releasing the previous buffer, which will happen when the page flip is done
            warn!(
                self.logger,
                "Tried to swap with an already queued flip"
            );
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
            let fb = framebuffer::create(::std::borrow::Borrow::borrow(&self.crtc), &*next_bo)
                .map_err(|_| SwapBuffersError::ContextLost)?;
            next_bo.set_userdata(fb).unwrap();
            fb
        };
        self.next_buffer.set(Some(next_bo));

        trace!(self.logger, "Queueing Page flip");
        ::std::borrow::Borrow::borrow(&self.crtc).page_flip(fb.handle())?;

        self.current_frame_buffer.set(fb);

        Ok(())
    }

    pub fn recreate<F>(&self, flip: F) -> Result<()>
    where
        F: FnOnce() -> ::std::result::Result<(), SwapBuffersError>
    {
        let (w, h) = self.pending_mode().size();

        // Recreate the surface and the related resources to match the new
        // resolution.
        debug!(
            self.logger,
            "Reinitializing surface for new mode: {}:{}", w, h
        );
        let surface = self
            .dev
            .borrow_mut()
            .create_surface(
                w as u32,
                h as u32,
                GbmFormat::XRGB8888,
                BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING,
            ).chain_err(|| ErrorKind::SurfaceCreationFailed)?;

        flip()?;

        // Clean up next_buffer
        {
            if let Some(mut old_bo) = self.next_buffer.take() {
                if let Ok(Some(fb)) = old_bo.take_userdata() {
                    if let Err(err) = framebuffer::destroy(::std::borrow::Borrow::borrow(&self.crtc), fb.handle()) {
                        warn!(
                            self.logger,
                            "Error releasing old back_buffer framebuffer: {:?}", err
                        );
                    }
                }
            }
        }

        // Cleanup front_buffer and init the first screen on the new front_buffer
        // (must be done before calling page_flip for the first time)
        let mut old_front_bo = self.front_buffer.replace({
            let mut front_bo = surface
                .lock_front_buffer()
                .chain_err(|| ErrorKind::FrontBufferLockFailed)?;

            debug!(
                self.logger,
                "FrontBuffer color format: {:?}",
                front_bo.format()
            );

            // we also need a new framebuffer for the front buffer
            let fb = framebuffer::create(::std::borrow::Borrow::borrow(&self.crtc), &*front_bo)
                .chain_err(|| ErrorKind::UnderlyingBackendError)?;

            ::std::borrow::Borrow::borrow(&self.crtc).commit(fb.handle())
                .chain_err(|| ErrorKind::UnderlyingBackendError)?;

            front_bo.set_userdata(fb).unwrap();
            front_bo
        });
        if let Ok(Some(fb)) = old_front_bo.take_userdata() {
            if let Err(err) = framebuffer::destroy(::std::borrow::Borrow::borrow(&self.crtc), fb.handle()) {
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

impl<D: RawDevice + 'static> Surface for GbmSurface<D>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>
{
    type Connectors = <<D as Device>::Surface as Surface>::Connectors;
    type Error = Error;

    fn crtc(&self) -> crtc::Handle {
        ::std::borrow::Borrow::borrow(&self.crtc)
            .crtc()
    }

    fn current_connectors(&self) -> Self::Connectors {
        ::std::borrow::Borrow::borrow(&self.crtc)
            .current_connectors()
    }
    
    fn pending_connectors(&self) -> Self::Connectors {
        ::std::borrow::Borrow::borrow(&self.crtc)
            .pending_connectors()
    }

    fn add_connector(&self, connector: connector::Handle) -> Result<()> {
        ::std::borrow::Borrow::borrow(&self.crtc)
            .add_connector(connector)
            .chain_err(|| ErrorKind::UnderlyingBackendError)
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<()> {
        ::std::borrow::Borrow::borrow(&self.crtc)
            .remove_connector(connector)
            .chain_err(|| ErrorKind::UnderlyingBackendError)
    }
    
    fn current_mode(&self) -> Mode {
        ::std::borrow::Borrow::borrow(&self.crtc)
            .current_mode()
    }
    
    fn pending_mode(&self) -> Mode {
        ::std::borrow::Borrow::borrow(&self.crtc)
            .pending_mode()
    }

    fn use_mode(&self, mode: Mode) -> Result<()> {
        ::std::borrow::Borrow::borrow(&self.crtc)
            .use_mode(mode)
            .chain_err(|| ErrorKind::UnderlyingBackendError)
    }
}

// FIXME:
//
// Option 1: When there is GAT support, impl `GraphicsBackend` for `LegacyDrmBackend`
//           using a new generic `B: Buffer` and use this:
/*
impl<'a, D: RawDevice + 'static> CursorBackend<'a> for GbmSurface<D>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>,
    <D as RawDevice>::Surface: CursorBackend<'a>,
    <<D as RawDevice>::Surface as CursorBackend<'a>>::CursorFormat: Buffer,
    <<D as RawDevice>::Surface as CursorBackend<'a>>::Error: ::std::error::Error + Send 
{
*/
//
// Option 2: When equality checks in where clauses are supported, we could at least do this:
/*
impl<'a, D: RawDevice + 'static> GraphicsBackend<'a> for GbmSurface<D>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>,
    <D as RawDevice>::Surface: CursorBackend<'a>,
    <<D as RawDevice>::Surface as CursorBackend<'a>>::CursorFormat=&'a Buffer,
    <<D as RawDevice>::Surface as CursorBackend<'a>>::Error: ::std::error::Error + Send 
{
*/
// But for now got to do this:

impl<'a, A: AsRawFd + 'static> CursorBackend<'a> for GbmSurface<LegacyDrmDevice<A>> {
    type CursorFormat = &'a ImageBuffer<Rgba<u8>, Vec<u8>>;
    type Error = Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<()> {
        ResultExt::chain_err(
            ::std::borrow::Borrow::<Rc<LegacyDrmSurface<A>>>::borrow(&self.crtc)
                .set_cursor_position(x, y),
            || ErrorKind::UnderlyingBackendError)
    }

    fn set_cursor_representation<'b>(
        &'b self,
        buffer: &ImageBuffer<Rgba<u8>, Vec<u8>>,
        hotspot: (u32, u32),
    ) -> Result<()>
        where 'a: 'b
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

        ResultExt::chain_err(
            ::std::borrow::Borrow::<Rc<LegacyDrmSurface<A>>>::borrow(&self.crtc)
                .set_cursor_representation(&cursor, hotspot),
            || ErrorKind::UnderlyingBackendError)?;

        // and store it
        self.cursor.set((cursor, hotspot));
        Ok(())
    }
}

impl<D: RawDevice + 'static> Drop for GbmSurface<D>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>
{
    fn drop(&mut self) {
        // Drop framebuffers attached to the userdata of the gbm surface buffers.
        // (They don't implement drop, as they need the device)
        if let Ok(Some(fb)) = {
            if let Some(mut next) = self.next_buffer.take() {
                next.take_userdata()
            } else if let Ok(mut next) = self.surface.borrow().lock_front_buffer() {
                next.take_userdata()
            } else {
                Ok(None)
            }
        } {
            // ignore failure at this point
            let _ = framebuffer::destroy(::std::borrow::Borrow::borrow(&self.crtc), fb.handle());
        }

        if let Ok(Some(fb)) = self.front_buffer.get_mut().take_userdata() {
            // ignore failure at this point
            let _ = framebuffer::destroy(::std::borrow::Borrow::borrow(&self.crtc), fb.handle());
        }
    }
}