use super::devices;
use super::error::*;
use backend::graphics::GraphicsBackend;
use backend::graphics::egl::{EGLGraphicsBackend, EGLSurface, PixelFormat, SwapBuffersError};
use drm::buffer::Buffer;
use drm::control::{connector, crtc, encoder, framebuffer, Mode};
use drm::control::ResourceInfo;
use gbm::{BufferObject, BufferObjectFlags, Format as GbmFormat, Surface as GbmSurface, SurfaceBufferHandle};
use image::{ImageBuffer, Rgba};
use nix::c_void;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Backend based on a `DrmDevice` and a given crtc
pub struct DrmBackend(Rc<RefCell<DrmBackendInternal>>);

impl DrmBackend {
    pub(crate) fn new(drm: Rc<RefCell<DrmBackendInternal>>) -> DrmBackend {
        DrmBackend(drm)
    }
}

/*
    Dependency graph
    - drm
        - gbm
            - context
            - gbm_surface
                - egl_surface
                - gbm_buffers
            - cursor
*/

pub(crate) struct GbmTypes<'dev, 'context> {
    cursor: BufferObject<'dev, ()>,
    surface: Surface<'context>,
}

pub(crate) struct EGL<'gbm, 'context> {
    surface: EGLSurface<'context, 'gbm, GbmSurface<'context, framebuffer::Info>>,
    buffers: GbmBuffers<'gbm>,
}

pub(crate) struct GbmBuffers<'gbm> {
    front_buffer: Cell<SurfaceBufferHandle<'gbm, framebuffer::Info>>,
    next_buffer: Cell<Option<SurfaceBufferHandle<'gbm, framebuffer::Info>>>,
}

rental! {
    mod graphics {
        use drm::control::framebuffer;
        use gbm::Surface as GbmSurface;
        use std::rc::Rc;
        use super::devices::{Context, Context_Borrow};
        use super::GbmTypes;

        #[rental]
        pub(crate) struct Surface<'context> {
            gbm: Box<GbmSurface<'context, framebuffer::Info>>,
            egl: super::EGL<'gbm, 'context>,
        }

        #[rental]
        pub(crate) struct Graphics {
            #[subrental(arity = 3)]
            context: Rc<Context>,
            gbm: GbmTypes<'context_1, 'context_2>,
        }
    }
}
use self::graphics::{Graphics, Surface};

/// Id of a `DrmBackend` related to its `DrmDevice`.
///
/// Used to track which `DrmBackend` finished page-flipping
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Id(usize);

impl Id {
    pub(crate) fn raw(&self) -> usize {
        self.0
    }
}

pub(crate) struct DrmBackendInternal {
    graphics: Graphics,
    crtc: crtc::Handle,
    mode: Mode,
    connectors: Vec<connector::Handle>,
    own_id: Id,
    logger: ::slog::Logger,
}

impl DrmBackendInternal {
    pub(crate) fn new<I, L>(context: Rc<devices::Context>, crtc: crtc::Handle, mode: Mode, connectors: I,
                            own_id: usize, logger: L)
                            -> Result<DrmBackendInternal>
    where
        I: Into<Vec<connector::Handle>>,
        L: Into<Option<::slog::Logger>>,
    {
        // logger already initialized by the DrmDevice
        let log = ::slog_or_stdlog(logger);
        info!(log, "Initializing DrmBackend");

        let connectors = connectors.into();

        // check the connectors, if they suite the mode
        for connector in connectors.iter() {
            if !connector::Info::load_from_device(context.head().head(), *connector)
                .chain_err(|| ErrorKind::DrmDev(format!("{:?}", context.head().head())))?
                .modes()
                .contains(&mode)
            {
                bail!(ErrorKind::ModeNotSuitable(mode))
            }
        }

        let (w, h) = mode.size();

        Ok(DrmBackendInternal {
            graphics: Graphics::try_new(context, |context| {
                Ok(GbmTypes {
                    cursor: {
                        // Create an unused cursor buffer (we don't want an Option here)
                        context
                            .devices
                            .gbm
                            .create_buffer_object(
                                1,
                                1,
                                GbmFormat::ARGB8888,
                                &[BufferObjectFlags::Cursor, BufferObjectFlags::Write],
                            )
                            .chain_err(|| ErrorKind::GbmInitFailed)?
                    },
                    surface: Surface::try_new(
                        {
                            debug!(log, "Creating GbmSurface");
                            // create a gbm surface
                            Box::new(context
                                .devices
                                .gbm
                                .create_surface(
                                    w as u32,
                                    h as u32,
                                    GbmFormat::XRGB8888,
                                    &[BufferObjectFlags::Scanout, BufferObjectFlags::Rendering],
                                )
                                .chain_err(|| ErrorKind::GbmInitFailed)?)
                        },
                        |surface| {
                            // create an egl surface from the gbm one
                            debug!(log, "Creating EGLSurface");
                            let egl_surface = context.egl.create_surface(&surface)?;

                            // make it active for the first `crtc::set`
                            // (which is needed before the first page_flip)
                            unsafe {
                                egl_surface
                                    .make_current()
                                    .chain_err(|| ErrorKind::FailedToSwap)?
                            };
                            egl_surface
                                .swap_buffers()
                                .chain_err(|| ErrorKind::FailedToSwap)?;

                            // init the first screen
                            // (must be done before calling page_flip for the first time)
                            let mut front_bo = surface
                                .lock_front_buffer()
                                .chain_err(|| ErrorKind::FailedToSwap)?;
                            debug!(log, "FrontBuffer color format: {:?}", front_bo.format());
                            // we need a framebuffer per front buffer
                            let fb = framebuffer::create(context.devices.drm, &*front_bo)
                                .chain_err(|| ErrorKind::DrmDev(format!("{:?}", context.devices.drm)))?;

                            debug!(log, "Initialize screen");
                            crtc::set(
                                context.devices.drm,
                                crtc,
                                fb.handle(),
                                &connectors,
                                (0, 0),
                                Some(mode),
                            ).chain_err(
                                || ErrorKind::DrmDev(format!("{:?}", context.devices.drm)),
                            )?;
                            front_bo.set_userdata(fb);

                            Ok(EGL {
                                surface: egl_surface,
                                buffers: GbmBuffers {
                                    front_buffer: Cell::new(front_bo),
                                    next_buffer: Cell::new(None),
                                },
                            })
                        },
                    ).map_err(Error::from)?,
                })
            })?,
            crtc,
            mode,
            connectors,
            own_id: Id(own_id),
            logger: log.clone(),
        })
    }

    pub(crate) fn unlock_buffer(&self) {
        // after the page swap is finished we need to release the rendered buffer.
        // this is called from the PageFlipHandler
        self.graphics.rent(|gbm| {
            gbm.surface.rent(|egl| {
                let next_bo = egl.buffers.next_buffer.replace(None);

                if let Some(next_buffer) = next_bo {
                    trace!(self.logger, "Releasing all front buffer");
                    egl.buffers.front_buffer.set(next_buffer);
                // drop and release the old buffer
                } else {
                    unreachable!();
                }
            })
        });
    }

    pub(crate) fn is_crtc(&self, crtc: crtc::Handle) -> bool {
        crtc == self.crtc
    }
}

impl DrmBackend {
    /// Add a connector to backend
    ///
    /// # Errors
    ///
    /// Errors if the new connector does not support the currently set `Mode`
    pub fn add_connector(&mut self, connector: connector::Handle) -> Result<()> {
        let info =
            connector::Info::load_from_device(self.0.borrow().graphics.head().head().head(), connector)
                .chain_err(|| {
                    ErrorKind::DrmDev(format!(
                        "{:?}",
                        self.0.borrow().graphics.head().head().head()
                    ))
                })?;

        // check if the connector can handle the current mode
        let mut internal = self.0.borrow_mut();
        if info.modes().contains(&internal.mode) {
            // check if there is a valid encoder
            let encoders = info.encoders()
                .iter()
                .map(|encoder| {
                    encoder::Info::load_from_device(self.0.borrow().graphics.head().head().head(), *encoder)
                        .chain_err(|| {
                            ErrorKind::DrmDev(format!(
                                "{:?}",
                                self.0.borrow().graphics.head().head().head()
                            ))
                        })
                })
                .collect::<Result<Vec<encoder::Info>>>()?;

            // and if any encoder supports the selected crtc
            if !encoders
                .iter()
                .any(|encoder| encoder.supports_crtc(self.0.borrow().crtc))
            {
                bail!(ErrorKind::NoSuitableEncoder(info, self.0.borrow().crtc));
            }

            info!(
                self.0.borrow().logger,
                "Adding new connector: {:?}",
                info.connector_type()
            );
            internal.connectors.push(connector);
            Ok(())
        } else {
            bail!(ErrorKind::ModeNotSuitable(self.0.borrow().mode))
        }
    }

    /// Returns a copy of the currently set connectors
    pub fn used_connectors(&self) -> Vec<connector::Handle> {
        // thanks to the RefCell we can sadly not return a `&[connector::Handle]`
        self.0.borrow().connectors.clone()
    }

    /// Removes a currently set connector
    pub fn remove_connector(&mut self, connector: connector::Handle) {
        if let Ok(info) =
            connector::Info::load_from_device(self.0.borrow().graphics.head().head().head(), connector)
        {
            info!(
                self.0.borrow().logger,
                "Removing connector: {:?}",
                info.connector_type()
            );
        } else {
            info!(self.0.borrow().logger, "Removing unknown connector");
        }

        self.0.borrow_mut().connectors.retain(|x| *x != connector);
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
        for connector in self.0.borrow().connectors.iter() {
            if !connector::Info::load_from_device(self.0.borrow().graphics.head().head().head(), *connector)
                .chain_err(|| {
                    ErrorKind::DrmDev(format!(
                        "{:?}",
                        self.0.borrow().graphics.head().head().head()
                    ))
                })?
                .modes()
                .contains(&mode)
            {
                bail!(ErrorKind::ModeNotSuitable(mode));
            }
        }

        // borrow & clone stuff because rust cannot figure out the upcoming
        // closure otherwise.
        let crtc = self.0.borrow().crtc;
        let mut internal = self.0.borrow_mut();
        let connectors = internal.connectors.clone();
        let logger = internal.logger.clone();

        let (w, h) = mode.size();

        internal.graphics.rent_all_mut(|graphics| -> Result<()> {
            // Recreate the surface and the related resources to match the new
            // resolution.
            debug!(logger, "Reinitializing surface for new mode: {}:{}", w, h);
            graphics.gbm.surface = Surface::try_new(
                {
                    // create a new gbm surface
                    debug!(logger, "Creating GbmSurface");
                    Box::new(graphics
                        .context
                        .devices
                        .gbm
                        .create_surface(
                            w as u32,
                            h as u32,
                            GbmFormat::XRGB8888,
                            &[BufferObjectFlags::Scanout, BufferObjectFlags::Rendering],
                        )
                        .chain_err(|| ErrorKind::GbmInitFailed)?)
                },
                |surface| {
                    // create an egl surface from the gbm one
                    debug!(logger, "Creating EGLSurface");
                    let egl_surface = graphics.context.egl.create_surface(&surface)?;

                    // make it active for the first `crtc::set`
                    // (which is needed before the first page_flip)
                    unsafe {
                        egl_surface
                            .make_current()
                            .chain_err(|| ErrorKind::FailedToSwap)?
                    };
                    egl_surface
                        .swap_buffers()
                        .chain_err(|| ErrorKind::FailedToSwap)?;

                    let mut front_bo = surface
                        .lock_front_buffer()
                        .chain_err(|| ErrorKind::FailedToSwap)?;
                    debug!(logger, "FrontBuffer color format: {:?}", front_bo.format());
                    // we need a framebuffer per front_buffer
                    let fb = framebuffer::create(graphics.context.devices.drm, &*front_bo).chain_err(|| {
                        ErrorKind::DrmDev(format!("{:?}", graphics.context.devices.drm))
                    })?;

                    debug!(logger, "Initialize screen");
                    crtc::set(
                        graphics.context.devices.drm,
                        crtc,
                        fb.handle(),
                        &connectors,
                        (0, 0),
                        Some(mode),
                    ).chain_err(|| {
                        ErrorKind::DrmDev(format!("{:?}", graphics.context.devices.drm))
                    })?;
                    front_bo.set_userdata(fb);

                    Ok(EGL {
                        surface: egl_surface,
                        buffers: GbmBuffers {
                            front_buffer: Cell::new(front_bo),
                            next_buffer: Cell::new(None),
                        },
                    })
                },
            )?;

            Ok(())
        })?;

        info!(logger, "Setting new mode: {:?}", mode.name());
        internal.mode = mode;
        Ok(())
    }

    /// Checks of the `DrmBackend` is of the given `Id`
    ///
    /// Only produces valid results, if the `Id` is from the `DrmDevice`,
    /// that created this backend.
    pub fn is(&self, id: Id) -> bool {
        self.0.borrow().own_id == id
    }
}

impl Drop for DrmBackend {
    fn drop(&mut self) {
        // Drop framebuffers attached to the userdata of the gbm surface buffers.
        // (They don't implement drop, as they need the device)
        self.0.borrow_mut().graphics.rent_all_mut(|graphics| {
            if let Some(fb) = graphics.gbm.surface.rent(|egl| {
                if let Some(mut next) = egl.buffers.next_buffer.take() {
                    return next.take_userdata();
                } else {
                    if let Ok(mut next) = graphics.gbm.surface.head().lock_front_buffer() {
                        return next.take_userdata();
                    }
                }
                None
            }) {
                // ignore failure at this point
                let _ = framebuffer::destroy(&*graphics.context.devices.drm, fb.handle());
            }

            if let Some(fb) = graphics.gbm.surface.rent_mut(|egl| {
                let first = egl.buffers.front_buffer.get_mut();
                first.take_userdata()
            }) {
                // ignore failure at this point
                let _ = framebuffer::destroy(&*graphics.context.devices.drm, fb.handle());
            }
        })
    }
}

impl GraphicsBackend for DrmBackend {
    type CursorFormat = ImageBuffer<Rgba<u8>, Vec<u8>>;
    type Error = Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<()> {
        trace!(self.0.borrow().logger, "Move the cursor to {},{}", x, y);
        crtc::move_cursor(
            self.0.borrow().graphics.head().head().head(),
            self.0.borrow().crtc,
            (x as i32, y as i32),
        ).chain_err(|| {
            ErrorKind::DrmDev(format!(
                "{:?}",
                self.0.borrow().graphics.head().head().head()
            ))
        })
    }

    fn set_cursor_representation(&self, buffer: ImageBuffer<Rgba<u8>, Vec<u8>>, hotspot: (u32, u32))
                                 -> Result<()> {
        let (w, h) = buffer.dimensions();
        debug!(self.0.borrow().logger, "Importing cursor");
        /// import the cursor into a buffer we can render
        self.0
            .borrow_mut()
            .graphics
            .rent_all_mut(|graphics| -> Result<()> {
                graphics.gbm.cursor = {
                    let mut cursor = graphics
                        .context
                        .devices
                        .gbm
                        .create_buffer_object(
                            w,
                            h,
                            GbmFormat::ARGB8888,
                            &[BufferObjectFlags::Cursor, BufferObjectFlags::Write],
                        )
                        .chain_err(|| ErrorKind::GbmInitFailed)?;
                    cursor
                        .write(&*buffer.into_raw())
                        .chain_err(|| ErrorKind::GbmInitFailed)?;
                    cursor
                };
                Ok(())
            })?;

        trace!(self.0.borrow().logger, "Set the new imported cursor");
        // and set it
        if crtc::set_cursor2(
            self.0.borrow().graphics.head().head().head(),
            self.0.borrow().crtc,
            self.0
                .borrow()
                .graphics
                .rent(|gbm| Buffer::handle(&gbm.cursor)),
            (w, h),
            (hotspot.0 as i32, hotspot.1 as i32),
        ).is_err()
        {
            crtc::set_cursor(
                self.0.borrow().graphics.head().head().head(),
                self.0.borrow().crtc,
                self.0
                    .borrow()
                    .graphics
                    .rent(|gbm| Buffer::handle(&gbm.cursor)),
                (w, h),
            ).chain_err(|| {
                ErrorKind::DrmDev(format!(
                    "{:?}",
                    self.0.borrow().graphics.head().head().head()
                ))
            })
        } else {
            Ok(())
        }
    }
}

impl EGLGraphicsBackend for DrmBackend {
    fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError> {
        self.0.borrow().graphics.rent_all(|graphics| {
            // We cannot call lock_front_buffer anymore without releasing the previous buffer, which will happen when the page flip is done
            if graphics.gbm.surface.rent(|egl| {
                let next = egl.buffers.next_buffer.take();
                let res = next.is_some();
                egl.buffers.next_buffer.set(next);
                res
            }) {
                warn!(self.0.borrow().logger, "Tried to swap a DrmBackend with a queued flip");
                return Err(SwapBuffersError::AlreadySwapped);
            }

            /// flip normally
            graphics.gbm.surface.rent(|egl| egl.surface.swap_buffers())?;

            graphics.gbm.surface.rent_all(|surface| {
                // supporting this error would cause a lot of inconvinience and
                // would most likely result in a lot of flickering.
                // neither weston, wlc or wlroots bother with that as well.
                // so we just assume we got at least two buffers to do flipping
                let mut next_bo = surface.gbm.lock_front_buffer().expect("Surface only has one front buffer. Not supported by smithay");

                // create a framebuffer if the front buffer does not have one already
                // (they are reused by gbm)
                let maybe_fb = next_bo.userdata().cloned();
                let fb = if let Some(info) = maybe_fb {
                    info
                } else {
                    let fb = framebuffer::create(graphics.context.devices.drm, &*next_bo).map_err(|_| SwapBuffersError::ContextLost)?;
                    next_bo.set_userdata(fb);
                    fb
                };
                surface.egl.buffers.next_buffer.set(Some(next_bo));

                trace!(self.0.borrow().logger, "Queueing Page flip");

                let id: Id = self.0.borrow().own_id;

                // and flip
                crtc::page_flip(graphics.context.devices.drm, self.0.borrow().crtc, fb.handle(), &[crtc::PageFlipFlags::PageFlipEvent], id).map_err(|_| SwapBuffersError::ContextLost)
            })
        })
    }

    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void {
        self.0
            .borrow()
            .graphics
            .head()
            .rent(|context| context.get_proc_address(symbol))
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        let (w, h) = self.0.borrow().mode.size();
        (w as u32, h as u32)
    }

    fn is_current(&self) -> bool {
        self.0
            .borrow()
            .graphics
            .head()
            .rent(|context| context.is_current())
    }

    unsafe fn make_current(&self) -> ::std::result::Result<(), SwapBuffersError> {
        self.0
            .borrow()
            .graphics
            .rent(|gbm| gbm.surface.rent(|egl| egl.surface.make_current()))
    }

    fn get_pixel_format(&self) -> PixelFormat {
        self.0
            .borrow()
            .graphics
            .head()
            .rent(|context| context.get_pixel_format())
    }
}
