use super::super::{Device, RawDevice, RawSurface, Surface};
use super::Error;

use drm::buffer::format::PixelFormat;
use drm::control::{connector, crtc, dumbbuffer::DumbBuffer, framebuffer, Device as ControlDevice, Mode};
#[cfg(feature = "backend_drm")]
use failure::ResultExt;
#[cfg(feature = "backend_drm")]
use image::{ImageBuffer, Rgba};
use nix::libc::{c_int, c_void};

use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::ptr;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[cfg(feature = "backend_drm")]
use crate::backend::drm::common::Error as DrmError;
#[cfg(feature = "backend_drm")]
use crate::backend::drm::DevPath;
use crate::backend::egl::ffi::{
    self,
    egl::{self, types::EGLStreamKHR},
};
use crate::backend::egl::{display::EGLDisplayHandle, wrap_egl_call, EGLError, SwapBuffersError};
#[cfg(feature = "backend_drm")]
use crate::backend::graphics::CursorBackend;

pub(in crate::backend::drm) struct EglStreamSurfaceInternal<D: RawDevice + 'static> {
    pub(in crate::backend::drm) crtc: <D as Device>::Surface,
    pub(in crate::backend::drm) cursor: Cell<Option<(DumbBuffer, (u32, u32))>>,
    pub(in crate::backend::drm) stream: RefCell<Option<(Arc<EGLDisplayHandle>, EGLStreamKHR)>>,
    pub(in crate::backend::drm) commit_buffer: Cell<Option<(DumbBuffer, framebuffer::Handle)>>,
    pub(in crate::backend::drm) locked: AtomicBool,
    pub(in crate::backend::drm) logger: ::slog::Logger,
}

impl<D: RawDevice + 'static> Surface for EglStreamSurfaceInternal<D> {
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

    fn use_mode(&self, mode: Mode) -> Result<(), Error<<<D as Device>::Surface as Surface>::Error>> {
        self.crtc.use_mode(mode).map_err(Error::Underlying)
    }
}

impl<D: RawDevice + 'static> Drop for EglStreamSurfaceInternal<D> {
    fn drop(&mut self) {
        if let Some((buffer, _)) = self.cursor.get() {
            let _ = self.crtc.destroy_dumb_buffer(buffer);
        }
        if let Some((buffer, fb)) = self.commit_buffer.take() {
            let _ = self.crtc.destroy_framebuffer(fb);
            let _ = self.crtc.destroy_dumb_buffer(buffer);
        }
        if let Some((display, stream)) = self.stream.replace(None) {
            unsafe {
                egl::DestroyStreamKHR(display.handle, stream);
            }
        }
    }
}

// Conceptionally EglStream is a weird api.
// It does modesetting on its own, bypassing our `RawSurface::commit` function.
// As a result, we cannot easily sync any other planes to the commit without more
// experimental egl extensions to do this via the EglStream-API.
//
// So instead we leverage the fact, that all drm-drivers still support the legacy
// `drmModeSetCursor` and `drmModeMoveCursor` functions, that (mostly) implicitly sync to the primary plane.
// That way we can use hardware cursors at least on all drm-backends (including atomic), although
// this is a little hacky. Overlay planes however are completely out of question for now.
//
// Note that this might still appear a little choppy, we should just use software cursors
// on eglstream devices by default and only use this, if the user really wants it.
#[cfg(feature = "backend_drm")]
impl<D: RawDevice + 'static> CursorBackend for EglStreamSurfaceInternal<D> {
    type CursorFormat = ImageBuffer<Rgba<u8>, Vec<u8>>;
    type Error = Error<DrmError>;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<(), Self::Error> {
        trace!(self.logger, "Move the cursor to {},{}", x, y);
        self.crtc
            .move_cursor(self.crtc.crtc(), (x as i32, y as i32))
            .compat()
            .map_err(|source| DrmError::Access {
                errmsg: "Error moving cursor",
                dev: self.crtc.dev_path(),
                source,
            })
            .map_err(Error::Underlying)
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
            .crtc
            .create_dumb_buffer((w, h), PixelFormat::ARGB8888)
            .compat()
            .map_err(Error::BufferCreationFailed)?;

        {
            let mut mapping = self
                .crtc
                .map_dumb_buffer(&mut cursor)
                .compat()
                .map_err(Error::BufferWriteFailed)?;
            mapping.as_mut().copy_from_slice(buffer);
        }

        trace!(self.logger, "Setting the new imported cursor");

        trace!(self.logger, "Setting the new imported cursor");

        // call the drm-functions directly to bypass ::commit/::page_flip and eglstream...

        if self
            .crtc
            .set_cursor2(
                self.crtc.crtc(),
                Some(&cursor),
                (hotspot.0 as i32, hotspot.1 as i32),
            )
            .is_err()
        {
            self.crtc
                .set_cursor(self.crtc.crtc(), Some(&cursor))
                .compat()
                .map_err(|source| DrmError::Access {
                    errmsg: "Failed to set cursor",
                    dev: self.crtc.dev_path(),
                    source,
                })
                .map_err(Error::Underlying)?;
        }

        // and store it
        if let Some((old, _)) = self.cursor.replace(Some((cursor, hotspot))) {
            if self.crtc.destroy_dumb_buffer(old).is_err() {
                warn!(self.logger, "Failed to free old cursor");
            }
        }

        Ok(())
    }
}

/// egl stream surface for rendering
pub struct EglStreamSurface<D: RawDevice + 'static>(
    pub(in crate::backend::drm) Rc<EglStreamSurfaceInternal<D>>,
);

impl<D: RawDevice + 'static> EglStreamSurface<D> {
    /// Check if underlying gbm resources need to be recreated.
    pub fn needs_recreation(&self) -> bool {
        self.0.crtc.commit_pending() || self.0.stream.borrow().is_none()
    }

    // An EGLStream is basically the pump that requests and consumes images to display them.
    // The heart of this weird api. Any changes to its configuration, require a re-creation.
    pub(super) fn create_stream(
        &self,
        display: &Arc<EGLDisplayHandle>,
        output_attribs: &[isize],
    ) -> Result<EGLStreamKHR, Error<<<D as Device>::Surface as Surface>::Error>> {
        // drop old steam, if it already exists
        if let Some((display, stream)) = self.0.stream.replace(None) {
            // ignore result
            unsafe {
                ffi::egl::DestroyStreamKHR(display.handle, stream);
            }
        }

        // because we are re-creating there might be a new mode. if there is? -> commit it
        if self.0.crtc.commit_pending() {
            let (w, h) = self.pending_mode().size();
            // but we need a buffer to commit...
            // well lets create one and clean it up once the stream is running
            if let Ok(buffer) = self
                .0
                .crtc
                .create_dumb_buffer((w as u32, h as u32), PixelFormat::ARGB8888)
            {
                if let Ok(fb) = self.0.crtc.add_framebuffer(&buffer) {
                    if let Some((buffer, fb)) = self.0.commit_buffer.replace(Some((buffer, fb))) {
                        let _ = self.0.crtc.destroy_framebuffer(fb);
                        let _ = self.0.crtc.destroy_dumb_buffer(buffer);
                    }
                    self.0.crtc.commit(fb).map_err(Error::Underlying)?;
                }
            }
        }

        // again enumerate extensions
        let extensions = {
            let p =
                unsafe { CStr::from_ptr(ffi::egl::QueryString(display.handle, ffi::egl::EXTENSIONS as i32)) };
            let list = String::from_utf8(p.to_bytes().to_vec()).unwrap_or_else(|_| String::new());
            list.split(' ').map(|e| e.to_string()).collect::<Vec<_>>()
        };

        // we need quite a bunch to implement a full-blown renderer.
        if !extensions.iter().any(|s| *s == "EGL_EXT_output_base")
            || !extensions.iter().any(|s| *s == "EGL_EXT_output_drm")
            || !extensions.iter().any(|s| *s == "EGL_KHR_stream")
            || !extensions
                .iter()
                .any(|s| *s == "EGL_EXT_stream_consumer_egloutput")
            || !extensions
                .iter()
                .any(|s| *s == "EGL_KHR_stream_producer_eglsurface")
        {
            error!(self.0.logger, "Extension for EGLStream surface creation missing");
            return Err(Error::DeviceIsNoEGLStreamDevice);
        }

        if cfg!(debug_assertions) {
            // TEST START
            let mut num_layers = 0;
            if unsafe {
                ffi::egl::GetOutputLayersEXT(
                    display.handle,
                    ptr::null(),
                    ptr::null_mut(),
                    10,
                    &mut num_layers,
                )
            } == 0
            {
                error!(self.0.logger, "Failed to get any! output layer");
            }
            if num_layers == 0 {
                error!(self.0.logger, "Failed to find any! output layer");
            }
            let mut layers = Vec::with_capacity(num_layers as usize);
            if unsafe {
                ffi::egl::GetOutputLayersEXT(
                    display.handle,
                    ptr::null(),
                    layers.as_mut_ptr(),
                    num_layers,
                    &mut num_layers,
                )
            } == 0
            {
                error!(self.0.logger, "Failed to receive Output Layers");
            }
            unsafe {
                layers.set_len(num_layers as usize);
            }
            for layer in layers {
                debug!(self.0.logger, "Found layer: {:?}", layer);
                let mut val = 0;
                if unsafe {
                    ffi::egl::QueryOutputLayerAttribEXT(
                        display.handle,
                        layer,
                        ffi::egl::DRM_CRTC_EXT as i32,
                        &mut val,
                    )
                } != 0
                {
                    info!(self.0.logger, "Possible crtc output layer: {}", val);
                }
                val = 0;
                if unsafe {
                    ffi::egl::QueryOutputLayerAttribEXT(
                        display.handle,
                        layer,
                        ffi::egl::DRM_PLANE_EXT as i32,
                        &mut val,
                    )
                } != 0
                {
                    info!(self.0.logger, "Possible plane output layer: {}", val);
                }
            }
            // TEST END
        }

        // alright, if the surface appears to be supported, we need an "output layer".
        // this is basically just a fancy name for a `crtc` or a `plane`.
        // those are exactly whats inside the `output_attribs` depending on underlying device.
        let mut num_layers = 0;
        if unsafe {
            ffi::egl::GetOutputLayersEXT(
                display.handle,
                output_attribs.as_ptr(),
                ptr::null_mut(),
                1,
                &mut num_layers,
            )
        } == 0
        {
            error!(
                self.0.logger,
                "Failed to acquire Output Layer. Attributes {:?}", output_attribs
            );
            return Err(Error::DeviceNoOutputLayer);
        }
        if num_layers == 0 {
            error!(self.0.logger, "Failed to find Output Layer");
            return Err(Error::DeviceNoOutputLayer);
        }
        let mut layers = Vec::with_capacity(num_layers as usize);
        if unsafe {
            ffi::egl::GetOutputLayersEXT(
                display.handle,
                output_attribs.as_ptr(),
                layers.as_mut_ptr(),
                num_layers,
                &mut num_layers,
            )
        } == 0
        {
            error!(self.0.logger, "Failed to get Output Layer");
            return Err(Error::DeviceNoOutputLayer);
        }
        unsafe {
            layers.set_len(num_layers as usize);
        }

        // lets just use the first layer and try to set the swap interval.
        // this is needed to make sure `eglSwapBuffers` does not block.
        let layer = layers[0];
        unsafe {
            ffi::egl::OutputLayerAttribEXT(display.handle, layer, ffi::egl::SWAP_INTERVAL_EXT as i32, 0);
        }

        // The stream needs to know, it needs to request frames
        // as soon as another one is rendered (we do not want to build a buffer and delay frames),
        // which is handled by STREAM_FIFO_LENGTH_KHR = 0.
        // We also want to "acquire" the frames manually. Like this we can request page-flip events
        // to drive our event loop. Otherwise we would have no way to know rendering is finished.
        let stream_attributes = {
            let mut out: Vec<c_int> = Vec::with_capacity(7);
            out.push(ffi::egl::STREAM_FIFO_LENGTH_KHR as i32);
            out.push(0);
            out.push(ffi::egl::CONSUMER_AUTO_ACQUIRE_EXT as i32);
            out.push(ffi::egl::FALSE as i32);
            out.push(ffi::egl::CONSUMER_ACQUIRE_TIMEOUT_USEC_KHR as i32);
            out.push(0);
            out.push(ffi::egl::NONE as i32);
            out
        };

        // okay, we have a config, lets create the stream.
        let stream = unsafe { ffi::egl::CreateStreamKHR(display.handle, stream_attributes.as_ptr()) };
        if stream == ffi::egl::NO_STREAM_KHR {
            error!(self.0.logger, "Failed to create egl stream");
            return Err(Error::DeviceStreamCreationFailed);
        }

        // we have a stream, lets connect it to our output layer
        if unsafe { ffi::egl::StreamConsumerOutputEXT(display.handle, stream, layer) } == 0 {
            error!(self.0.logger, "Failed to link Output Layer as Stream Consumer");
            return Err(Error::DeviceStreamCreationFailed);
        }

        let _ = self.0.stream.replace(Some((display.clone(), stream)));

        Ok(stream)
    }

    pub(super) fn create_surface(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
        _surface_attribs: &[c_int],
        output_attribs: &[isize],
    ) -> Result<*const c_void, Error<<<D as Device>::Surface as Surface>::Error>> {
        // our surface needs a stream
        let stream = self.create_stream(display, output_attribs)?;

        let (w, h) = self.current_mode().size();
        info!(self.0.logger, "Creating stream surface with size: ({}:{})", w, h);
        let surface_attributes = {
            let mut out: Vec<c_int> = Vec::with_capacity(5);
            out.push(ffi::egl::WIDTH as i32);
            out.push(w as i32);
            out.push(ffi::egl::HEIGHT as i32);
            out.push(h as i32);
            out.push(ffi::egl::NONE as i32);
            out
        };

        // the stream is already connected to the consumer (output layer) during creation.
        // we now connect the producer (out egl surface, that we render to).
        let surface = unsafe {
            ffi::egl::CreateStreamProducerSurfaceKHR(
                display.handle,
                config_id,
                stream,
                surface_attributes.as_ptr(),
            )
        };
        if surface == ffi::egl::NO_SURFACE {
            error!(self.0.logger, "Failed to create surface: 0x{:X}", unsafe {
                ffi::egl::GetError()
            });
        }
        Ok(surface)
    }

    pub(super) fn flip(
        &self,
        crtc: crtc::Handle,
        display: &Arc<EGLDisplayHandle>,
        surface: ffi::egl::types::EGLSurface,
    ) -> Result<(), SwapBuffersError<Error<<<D as Device>::Surface as Surface>::Error>>> {
        // if we have already swapped the buffer successfully, we need to free it again.
        //
        // we need to do this here, because the call may fail (compare this with gbm's unlock_buffer...).
        // if it fails we do not want to swap, because that would block and as a result deadlock us.
        if self.0.locked.load(Ordering::SeqCst) {
            // which means in eglstream terms: "acquire it".
            // here we set the user data of the page_flip event
            // (which is also only triggered if we manually acquire frames).
            // This is the crtc id like always to get the matching surface for the device handler later.
            let acquire_attributes = [
                ffi::egl::DRM_FLIP_EVENT_DATA_NV as isize,
                Into::<u32>::into(crtc) as isize,
                ffi::egl::NONE as isize,
            ];

            if let Ok(stream) = self.0.stream.try_borrow() {
                // lets try to acquire the frame.
                // this may fail, if the buffer is still in use by the gpu,
                // e.g. the flip was not done yet. In this case this call fails as `BUSY`.
                let res = if let Some(&(ref display, ref stream)) = stream.as_ref() {
                    wrap_egl_call(|| unsafe {
                        ffi::egl::StreamConsumerAcquireAttribNV(
                            display.handle,
                            *stream,
                            acquire_attributes.as_ptr(),
                        );
                    })
                    .map_err(Error::StreamFlipFailed)
                } else {
                    Err(Error::StreamFlipFailed(EGLError::NotInitialized))
                };

                // so we need to unlock on success and return on failure.
                if res.is_ok() {
                    self.0.locked.store(false, Ordering::SeqCst);
                } else {
                    return res.map_err(SwapBuffersError::Underlying);
                }
            }
        }

        // so if we are not locked any more we can send the next frame by calling swap buffers.
        if !self.0.locked.load(Ordering::SeqCst) {
            wrap_egl_call(|| unsafe { ffi::egl::SwapBuffers(***display, surface as *const _) })
                .map_err(SwapBuffersError::EGLSwapBuffers)?;
            self.0.locked.store(true, Ordering::SeqCst);
        }

        Ok(())
    }
}

impl<D: RawDevice + 'static> Surface for EglStreamSurface<D> {
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

#[cfg(feature = "backend_drm_legacy")]
impl<D: RawDevice + 'static> CursorBackend for EglStreamSurface<D> {
    type CursorFormat = ImageBuffer<Rgba<u8>, Vec<u8>>;
    type Error = Error<DrmError>;

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
