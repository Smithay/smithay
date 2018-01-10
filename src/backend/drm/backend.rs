use super::DevPath;
use super::error::*;
use backend::graphics::GraphicsBackend;
use backend::graphics::egl::{EGLContext, EGLGraphicsBackend, EGLSurface, PixelFormat, SwapBuffersError};
use backend::graphics::egl::error::Result as EGLResult;
use backend::graphics::egl::native::{Gbm, GbmSurfaceArguments};
use backend::graphics::egl::wayland::{EGLDisplay, EGLWaylandExtensions};
use drm::control::{Device, ResourceInfo};
use drm::control::{connector, crtc, encoder, framebuffer, Mode};
use gbm::{BufferObject, BufferObjectFlags, Device as GbmDevice, Format as GbmFormat, Surface as GbmSurface,
          SurfaceBufferHandle};
use image::{ImageBuffer, Rgba};
use nix::libc::c_void;
use std::cell::Cell;
use std::rc::{Rc, Weak};
use wayland_server::Display;

/// Backend based on a `DrmDevice` and a given crtc
pub struct DrmBackend<A: Device + 'static> {
    backend: Rc<DrmBackendInternal<A>>,
    surface: EGLSurface<GbmSurface<framebuffer::Info>>,
    mode: Mode,
    connectors: Vec<connector::Handle>,
}

pub(crate) struct DrmBackendInternal<A: Device + 'static> {
    context: Rc<EGLContext<Gbm<framebuffer::Info>, GbmDevice<A>>>,
    cursor: Cell<BufferObject<()>>,
    current_frame_buffer: Cell<framebuffer::Info>,
    front_buffer: Cell<SurfaceBufferHandle<framebuffer::Info>>,
    next_buffer: Cell<Option<SurfaceBufferHandle<framebuffer::Info>>>,
    crtc: crtc::Handle,
    logger: ::slog::Logger,
}

impl<A: Device + 'static> DrmBackend<A> {
    pub(crate) fn new(
        context: Rc<EGLContext<Gbm<framebuffer::Info>, GbmDevice<A>>>, crtc: crtc::Handle, mode: Mode,
        connectors: Vec<connector::Handle>, log: ::slog::Logger,
    ) -> Result<Self> {
        // logger already initialized by the DrmDevice
        info!(log, "Initializing DrmBackend");

        let (w, h) = mode.size();

        debug!(log, "Creating Surface");
        let surface = context
            .create_surface(GbmSurfaceArguments {
                size: (w as u32, h as u32),
                format: GbmFormat::XRGB8888,
                flags: BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING,
            })
            .chain_err(|| ErrorKind::GbmInitFailed)?;

        // make it active for the first `crtc::set`
        // (which is needed before the first page_flip)
        unsafe {
            surface
                .make_current()
                .chain_err(|| ErrorKind::FailedToSwap)?
        };
        surface
            .swap_buffers()
            .chain_err(|| ErrorKind::FailedToSwap)?;

        // init the first screen
        // (must be done before calling page_flip for the first time)
        let mut front_bo = surface
            .lock_front_buffer()
            .chain_err(|| ErrorKind::FailedToSwap)?;

        debug!(log, "FrontBuffer color format: {:?}", front_bo.format());

        // we need a framebuffer for the front buffer
        let fb = framebuffer::create(&*context, &*front_bo).chain_err(|| {
            ErrorKind::DrmDev(format!(
                "Error creating framebuffer on {:?}",
                context.dev_path()
            ))
        })?;

        debug!(log, "Initialize screen");
        crtc::set(
            &*context,
            crtc,
            fb.handle(),
            &connectors,
            (0, 0),
            Some(mode),
        ).chain_err(|| {
            ErrorKind::DrmDev(format!(
                "Error setting crtc {:?} on {:?}",
                crtc,
                context.dev_path()
            ))
        })?;
        front_bo.set_userdata(fb).unwrap();

        let cursor = Cell::new(context
            .create_buffer_object(
                1,
                1,
                GbmFormat::ARGB8888,
                BufferObjectFlags::CURSOR | BufferObjectFlags::WRITE,
            )
            .chain_err(|| ErrorKind::GbmInitFailed)?);

        Ok(DrmBackend {
            backend: Rc::new(DrmBackendInternal {
                context,
                cursor,
                current_frame_buffer: Cell::new(fb),
                front_buffer: Cell::new(front_bo),
                next_buffer: Cell::new(None),
                crtc,
                logger: log,
            }),
            surface,
            mode,
            connectors,
        })
    }

    pub(crate) fn weak(&self) -> Weak<DrmBackendInternal<A>> {
        Rc::downgrade(&self.backend)
    }

    /// Add a connector to backend
    ///
    /// # Errors
    ///
    /// Errors if the new connector does not support the currently set `Mode`
    pub fn add_connector(&mut self, connector: connector::Handle) -> Result<()> {
        let info = connector::Info::load_from_device(&*self.backend.context, connector).chain_err(|| {
            ErrorKind::DrmDev(format!(
                "Error loading connector info on {:?}",
                self.backend.context.dev_path()
            ))
        })?;

        // check if the connector can handle the current mode
        if info.modes().contains(&self.mode) {
            // check if there is a valid encoder
            let encoders = info.encoders()
                .iter()
                .map(|encoder| {
                    encoder::Info::load_from_device(&*self.backend.context, *encoder).chain_err(|| {
                        ErrorKind::DrmDev(format!(
                            "Error loading encoder info on {:?}",
                            self.backend.context.dev_path()
                        ))
                    })
                })
                .collect::<Result<Vec<encoder::Info>>>()?;

            // and if any encoder supports the selected crtc
            let resource_handles = self.backend.context.resource_handles().chain_err(|| {
                ErrorKind::DrmDev(format!(
                    "Error loading resources on {:?}",
                    self.backend.context.dev_path()
                ))
            })?;
            if !encoders
                .iter()
                .map(|encoder| encoder.possible_crtcs())
                .all(|crtc_list| {
                    resource_handles
                        .filter_crtcs(crtc_list)
                        .contains(&self.backend.crtc)
                }) {
                bail!(ErrorKind::NoSuitableEncoder(info, self.backend.crtc));
            }

            info!(
                self.backend.logger,
                "Adding new connector: {:?}",
                info.connector_type()
            );
            self.connectors.push(connector);
            Ok(())
        } else {
            bail!(ErrorKind::ModeNotSuitable(self.mode))
        }
    }

    /// Returns the currently set connectors
    pub fn used_connectors(&self) -> &[connector::Handle] {
        &*self.connectors
    }

    /// Removes a currently set connector
    pub fn remove_connector(&mut self, connector: connector::Handle) {
        if let Ok(info) = connector::Info::load_from_device(&*self.backend.context, connector) {
            info!(
                self.backend.logger,
                "Removing connector: {:?}",
                info.connector_type()
            );
        } else {
            info!(self.backend.logger, "Removing unknown connector");
        }

        self.connectors.retain(|x| *x != connector);
    }

    /// Changes the currently set mode
    ///
    /// # Errors
    ///
    /// This will fail if not all set connectors support the new `Mode`.
    /// Several internal resources will need to be recreated to fit the new `Mode`.
    /// Other errors might occur.
    pub fn use_mode(&mut self, mode: Mode) -> Result<()> {
        // check the connectors
        for connector in &self.connectors {
            if !connector::Info::load_from_device(&*self.backend.context, *connector)
                .chain_err(|| {
                    ErrorKind::DrmDev(format!(
                        "Error loading connector info on {:?}",
                        self.backend.context.dev_path()
                    ))
                })?
                .modes()
                .contains(&mode)
            {
                bail!(ErrorKind::ModeNotSuitable(mode));
            }
        }

        info!(self.backend.logger, "Setting new mode: {:?}", mode.name());
        let (w, h) = mode.size();

        // Recreate the surface and the related resources to match the new
        // resolution.
        debug!(
            self.backend.logger,
            "Reinitializing surface for new mode: {}:{}", w, h
        );
        let surface = self.backend
            .context
            .create_surface(GbmSurfaceArguments {
                size: (w as u32, h as u32),
                format: GbmFormat::XRGB8888,
                flags: BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING,
            })
            .chain_err(|| ErrorKind::GbmInitFailed)?;

        // make it active for the first `crtc::set`
        // (which is needed before the first page_flip)
        unsafe {
            surface
                .make_current()
                .chain_err(|| ErrorKind::FailedToSwap)?
        };
        surface
            .swap_buffers()
            .chain_err(|| ErrorKind::FailedToSwap)?;

        // Clean up next_buffer
        {
            if let Some(mut old_bo) = self.backend.next_buffer.take() {
                if let Ok(Some(fb)) = old_bo.take_userdata() {
                    if let Err(err) = framebuffer::destroy(&*self.backend.context, fb.handle()) {
                        warn!(
                            self.backend.logger,
                            "Error releasing old back_buffer framebuffer: {:?}", err
                        );
                    }
                }
            }
        }

        // Cleanup front_buffer and init the first screen on the new front_buffer
        // (must be done before calling page_flip for the first time)
        let mut old_front_bo = self.backend.front_buffer.replace({
            let mut front_bo = surface
                .lock_front_buffer()
                .chain_err(|| ErrorKind::FailedToSwap)?;

            debug!(
                self.backend.logger,
                "FrontBuffer color format: {:?}",
                front_bo.format()
            );

            // we also need a new framebuffer for the front buffer
            let dev_path = self.backend.context.dev_path();
            let fb = framebuffer::create(&*self.backend.context, &*front_bo)
                .chain_err(|| ErrorKind::DrmDev(format!("Error creating framebuffer on {:?}", dev_path)))?;

            front_bo.set_userdata(fb).unwrap();

            debug!(self.backend.logger, "Setting screen");
            crtc::set(
                &*self.backend.context,
                self.backend.crtc,
                fb.handle(),
                &self.connectors,
                (0, 0),
                Some(mode),
            ).chain_err(|| {
                ErrorKind::DrmDev(format!(
                    "Error setting crtc {:?} on {:?}",
                    self.backend.crtc,
                    self.backend.context.dev_path()
                ))
            })?;

            front_bo
        });
        if let Ok(Some(fb)) = old_front_bo.take_userdata() {
            if let Err(err) = framebuffer::destroy(&*self.backend.context, fb.handle()) {
                warn!(
                    self.backend.logger,
                    "Error releasing old front_buffer framebuffer: {:?}", err
                );
            }
        }

        // Drop the old surface after cleanup
        self.surface = surface;
        self.mode = mode;
        Ok(())
    }

    /// Returns the crtc id used by this backend
    pub fn crtc(&self) -> crtc::Handle {
        self.backend.crtc
    }
}

impl<A: Device + 'static> DrmBackendInternal<A> {
    pub(crate) fn unlock_buffer(&self) {
        // after the page swap is finished we need to release the rendered buffer.
        // this is called from the PageFlipHandler
        if let Some(next_buffer) = self.next_buffer.replace(None) {
            trace!(self.logger, "Releasing old front buffer");
            self.front_buffer.set(next_buffer);
            // drop and release the old buffer
        }
    }

    pub(crate) fn page_flip(
        &self, fb: Option<&framebuffer::Info>
    ) -> ::std::result::Result<(), SwapBuffersError> {
        trace!(self.logger, "Queueing Page flip");

        let fb = *fb.unwrap_or(&self.current_frame_buffer.get());

        // and flip
        crtc::page_flip(
            &*self.context,
            self.crtc,
            fb.handle(),
            &[crtc::PageFlipFlags::PageFlipEvent],
        ).map_err(|_| SwapBuffersError::ContextLost)?;

        self.current_frame_buffer.set(fb);

        Ok(())
    }
}

impl<A: Device + 'static> Drop for DrmBackend<A> {
    fn drop(&mut self) {
        // Drop framebuffers attached to the userdata of the gbm surface buffers.
        // (They don't implement drop, as they need the device)
        if let Ok(Some(fb)) = {
            if let Some(mut next) = self.backend.next_buffer.take() {
                next.take_userdata()
            } else if let Ok(mut next) = self.surface.lock_front_buffer() {
                next.take_userdata()
            } else {
                Ok(None)
            }
        } {
            // ignore failure at this point
            let _ = framebuffer::destroy(&*self.backend.context, fb.handle());
        }
    }
}

impl<A: Device + 'static> Drop for DrmBackendInternal<A> {
    fn drop(&mut self) {
        if let Ok(Some(fb)) = self.front_buffer.get_mut().take_userdata() {
            // ignore failure at this point
            let _ = framebuffer::destroy(&*self.context, fb.handle());
        }

        // ignore failure at this point
        let _ = crtc::clear_cursor(&*self.context, self.crtc);
    }
}

impl<A: Device + 'static> GraphicsBackend for DrmBackend<A> {
    type CursorFormat = ImageBuffer<Rgba<u8>, Vec<u8>>;
    type Error = Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<()> {
        trace!(self.backend.logger, "Move the cursor to {},{}", x, y);
        crtc::move_cursor(
            &*self.backend.context,
            self.backend.crtc,
            (x as i32, y as i32),
        ).chain_err(|| {
            ErrorKind::DrmDev(format!(
                "Error moving cursor on {:?}",
                self.backend.context.dev_path()
            ))
        })
    }

    fn set_cursor_representation(
        &self, buffer: &ImageBuffer<Rgba<u8>, Vec<u8>>, hotspot: (u32, u32)
    ) -> Result<()> {
        let (w, h) = buffer.dimensions();
        debug!(self.backend.logger, "Importing cursor");

        // import the cursor into a buffer we can render
        let mut cursor = self.backend
            .context
            .create_buffer_object(
                w,
                h,
                GbmFormat::ARGB8888,
                BufferObjectFlags::CURSOR | BufferObjectFlags::WRITE,
            )
            .chain_err(|| ErrorKind::GbmInitFailed)?;
        cursor
            .write(&**buffer)
            .chain_err(|| ErrorKind::GbmInitFailed)?
            .chain_err(|| ErrorKind::GbmInitFailed)?;

        trace!(self.backend.logger, "Setting the new imported cursor");

        // and set it
        if crtc::set_cursor2(
            &*self.backend.context,
            self.backend.crtc,
            &cursor,
            (hotspot.0 as i32, hotspot.1 as i32),
        ).is_err()
        {
            crtc::set_cursor(&*self.backend.context, self.backend.crtc, &cursor).chain_err(|| {
                ErrorKind::DrmDev(format!(
                    "Failed to set cursor on {:?}",
                    self.backend.context.dev_path()
                ))
            })?;
        }

        // and store it
        self.backend.cursor.set(cursor);
        Ok(())
    }
}

impl<A: Device + 'static> EGLGraphicsBackend for DrmBackend<A> {
    fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError> {
        if {
            let nb = self.backend.next_buffer.take();
            let res = nb.is_some();
            self.backend.next_buffer.set(nb);
            res
        } {
            // We cannot call lock_front_buffer anymore without releasing the previous buffer, which will happen when the page flip is done
            warn!(
                self.backend.logger,
                "Tried to swap a DrmBackend with a queued flip"
            );
            return Err(SwapBuffersError::AlreadySwapped);
        }

        // flip normally
        self.surface.swap_buffers()?;

        // supporting only one buffer would cause a lot of inconvinience and
        // would most likely result in a lot of flickering.
        // neither weston, wlc or wlroots bother with that as well.
        // so we just assume we got at least two buffers to do flipping.
        let mut next_bo = self.surface
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
            let fb = framebuffer::create(&*self.backend.context, &*next_bo)
                .map_err(|_| SwapBuffersError::ContextLost)?;
            next_bo.set_userdata(fb).unwrap();
            fb
        };
        self.backend.next_buffer.set(Some(next_bo));

        self.backend.page_flip(Some(&fb))
    }

    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void {
        self.backend.context.get_proc_address(symbol)
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        let (w, h) = self.mode.size();
        (w as u32, h as u32)
    }

    fn is_current(&self) -> bool {
        self.backend.context.is_current() && self.surface.is_current()
    }

    unsafe fn make_current(&self) -> ::std::result::Result<(), SwapBuffersError> {
        self.surface.make_current()
    }

    fn get_pixel_format(&self) -> PixelFormat {
        self.backend.context.get_pixel_format()
    }
}

impl<A: Device + 'static> EGLWaylandExtensions for DrmBackend<A> {
    fn bind_wl_display(&self, display: &Display) -> EGLResult<EGLDisplay> {
        self.backend.context.bind_wl_display(display)
    }
}
