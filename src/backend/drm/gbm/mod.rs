//!
//! [`Device`](Device) and [`Surface`](Surface)
//! implementations using gbm buffers for efficient rendering.
//!
//! Usually this implementation will be wrapped into a [`EglDevice`](::backend::drm::egl::EglDevice).
//! Take a look at `anvil`s source code for an example of this.
//!
//! To use these types standalone, you will need to consider the special requirements
//! of [`GbmSurface::page_flip`](::backend::drm::gbm::GbmSurface::page_flip).
//!

use super::{Device, DeviceHandler, RawDevice, ResourceHandles, Surface};
use crate::backend::graphics::SwapBuffersError;

use drm::control::{connector, crtc, encoder, framebuffer, plane, Device as ControlDevice, Mode};
use drm::SystemError as DrmError;
use gbm::{self, BufferObjectFlags, Format as GbmFormat};
use nix::libc::dev_t;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use std::sync::Once;

/// Errors thrown by the [`GbmDevice`](::backend::drm::gbm::GbmDevice)
/// and [`GbmSurface`](::backend::drm::gbm::GbmSurface).
#[derive(thiserror::Error, Debug)]
pub enum Error<U: std::error::Error + 'static> {
    /// Creation of GBM device failed
    #[error("Creation of GBM device failed")]
    InitFailed(#[source] io::Error),
    /// Creation of GBM surface failed
    #[error("Creation of GBM surface failed")]
    SurfaceCreationFailed(#[source] io::Error),
    /// Creation of GBM buffer object failed
    #[error("Creation of GBM buffer object failed")]
    BufferCreationFailed(#[source] io::Error),
    /// Writing to GBM buffer failed
    #[error("Writing to GBM buffer failed")]
    BufferWriteFailed(#[source] io::Error),
    /// Creation of drm framebuffer failed
    #[error("Creation of drm framebuffer failed")]
    FramebufferCreationFailed(#[source] failure::Compat<drm::SystemError>),
    /// Lock of GBM surface front buffer failed
    #[error("Lock of GBM surface font buffer failed")]
    FrontBufferLockFailed,
    /// No additional buffers are available
    #[error("No additional buffers are available. Did you swap twice?")]
    FrontBuffersExhausted,
    /// Internal state was modified
    #[error("Internal state was modified. Did you change gbm userdata?")]
    InvalidInternalState,
    /// The GBM device was destroyed
    #[error("The GBM device was destroyed")]
    DeviceDestroyed,
    /// Underlying backend error
    #[error("Underlying error: {0}")]
    Underlying(#[source] U),
}

mod surface;
pub use self::surface::GbmSurface;
use self::surface::GbmSurfaceInternal;

#[cfg(feature = "backend_egl")]
pub mod egl;

#[cfg(feature = "backend_session")]
pub mod session;

static LOAD: Once = Once::new();

/// Representation of an open gbm device to create rendering surfaces
pub struct GbmDevice<D: RawDevice + ControlDevice + 'static> {
    pub(self) dev: Rc<RefCell<gbm::Device<D>>>,
    backends: Rc<RefCell<HashMap<crtc::Handle, Weak<GbmSurfaceInternal<D>>>>>,
    logger: ::slog::Logger,
}

impl<D: RawDevice + ControlDevice + 'static> GbmDevice<D> {
    /// Create a new [`GbmDevice`] from an open drm node
    ///
    /// Returns an error if the file is no valid drm node or context creation was not
    /// successful.
    pub fn new<L>(mut dev: D, logger: L) -> Result<Self, Error<<<D as Device>::Surface as Surface>::Error>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        /* GBM will load a dri driver, but even though they need symbols from
         * libglapi, in some version of Mesa they are not linked to it. Since
         * only the gl-renderer module links to it, these symbols won't be
         * globally available, and loading the DRI driver fails.
         * Workaround this by dlopen()'ing libglapi with RTLD_GLOBAL.
         */
        LOAD.call_once(|| unsafe {
            nix::libc::dlopen(
                "libglapi.so.0".as_ptr() as *const _,
                nix::libc::RTLD_LAZY | nix::libc::RTLD_GLOBAL,
            );
        });

        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_gbm"));

        dev.clear_handler();

        debug!(log, "Creating gbm device");
        Ok(GbmDevice {
            // Open the gbm device from the drm device
            dev: Rc::new(RefCell::new(gbm::Device::new(dev).map_err(Error::InitFailed)?)),
            backends: Rc::new(RefCell::new(HashMap::new())),
            logger: log,
        })
    }
}

struct InternalDeviceHandler<D: RawDevice + ControlDevice + 'static> {
    handler: Box<dyn DeviceHandler<Device = GbmDevice<D>> + 'static>,
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<GbmSurfaceInternal<D>>>>>,
    logger: ::slog::Logger,
}

impl<D: RawDevice + ControlDevice + 'static> DeviceHandler for InternalDeviceHandler<D> {
    type Device = D;

    fn vblank(&mut self, crtc: crtc::Handle) {
        if let Some(backends) = self.backends.upgrade() {
            if let Some(surface) = backends.borrow().get(&crtc) {
                if let Some(surface) = surface.upgrade() {
                    surface.unlock_buffer();
                    self.handler.vblank(crtc);
                }
            } else {
                warn!(
                    self.logger,
                    "Surface ({:?}) not managed by gbm, event not handled.", crtc
                );
            }
        }
    }
    fn error(&mut self, error: <<D as Device>::Surface as Surface>::Error) {
        self.handler.error(Error::Underlying(error))
    }
}

impl<D: RawDevice + ControlDevice + 'static> Device for GbmDevice<D> {
    type Surface = GbmSurface<D>;

    fn device_id(&self) -> dev_t {
        self.dev.borrow().device_id()
    }

    fn set_handler(&mut self, handler: impl DeviceHandler<Device = Self> + 'static) {
        self.dev.borrow_mut().set_handler(InternalDeviceHandler {
            handler: Box::new(handler),
            backends: Rc::downgrade(&self.backends),
            logger: self.logger.clone(),
        });
    }

    fn clear_handler(&mut self) {
        self.dev.borrow_mut().clear_handler();
    }

    fn create_surface(
        &mut self,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
    ) -> Result<GbmSurface<D>, Error<<<D as Device>::Surface as Surface>::Error>> {
        info!(self.logger, "Initializing GbmSurface");

        let drm_surface = Device::create_surface(&mut **self.dev.borrow_mut(), crtc, mode, connectors)
            .map_err(Error::Underlying)?;

        // initialize the surface
        let (w, h) = drm_surface.pending_mode().size();
        let surface = self
            .dev
            .borrow()
            .create_surface(
                w as u32,
                h as u32,
                GbmFormat::XRGB8888,
                BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING,
            )
            .map_err(Error::SurfaceCreationFailed)?;

        // initialize a buffer for the cursor image
        let cursor = Cell::new((
            self.dev
                .borrow()
                .create_buffer_object(
                    1,
                    1,
                    GbmFormat::ARGB8888,
                    BufferObjectFlags::CURSOR | BufferObjectFlags::WRITE,
                )
                .map_err(Error::BufferCreationFailed)?,
            (0, 0),
        ));

        let backend = Rc::new(GbmSurfaceInternal {
            dev: self.dev.clone(),
            surface: RefCell::new(surface),
            crtc: drm_surface,
            cursor,
            current_frame_buffer: Cell::new(None),
            front_buffer: Cell::new(None),
            next_buffer: Cell::new(None),
            recreated: Cell::new(true),
            logger: self.logger.new(o!("crtc" => format!("{:?}", crtc))),
        });
        self.backends.borrow_mut().insert(crtc, Rc::downgrade(&backend));
        Ok(GbmSurface(backend))
    }

    fn process_events(&mut self) {
        self.dev.borrow_mut().process_events()
    }

    fn resource_handles(&self) -> Result<ResourceHandles, Error<<<D as Device>::Surface as Surface>::Error>> {
        Device::resource_handles(&**self.dev.borrow()).map_err(Error::Underlying)
    }

    fn get_connector_info(&self, conn: connector::Handle) -> std::result::Result<connector::Info, DrmError> {
        self.dev.borrow().get_connector_info(conn)
    }
    fn get_crtc_info(&self, crtc: crtc::Handle) -> std::result::Result<crtc::Info, DrmError> {
        self.dev.borrow().get_crtc_info(crtc)
    }
    fn get_encoder_info(&self, enc: encoder::Handle) -> std::result::Result<encoder::Info, DrmError> {
        self.dev.borrow().get_encoder_info(enc)
    }
    fn get_framebuffer_info(
        &self,
        fb: framebuffer::Handle,
    ) -> std::result::Result<framebuffer::Info, DrmError> {
        self.dev.borrow().get_framebuffer_info(fb)
    }
    fn get_plane_info(&self, plane: plane::Handle) -> std::result::Result<plane::Info, DrmError> {
        self.dev.borrow().get_plane_info(plane)
    }
}

impl<D: RawDevice + ControlDevice + 'static> AsRawFd for GbmDevice<D> {
    fn as_raw_fd(&self) -> RawFd {
        self.dev.borrow().as_raw_fd()
    }
}

impl<D: RawDevice + ControlDevice + 'static> Drop for GbmDevice<D> {
    fn drop(&mut self) {
        self.clear_handler();
    }
}

impl<E> Into<SwapBuffersError> for Error<E>
where
    E: std::error::Error + Into<SwapBuffersError> + 'static,
{
    fn into(self) -> SwapBuffersError {
        match self {
            Error::FrontBuffersExhausted => SwapBuffersError::AlreadySwapped,
            Error::FramebufferCreationFailed(x)
                if match x.get_ref() {
                    drm::SystemError::Unknown {
                        errno: nix::errno::Errno::EBUSY,
                    } => true,
                    drm::SystemError::Unknown {
                        errno: nix::errno::Errno::EINTR,
                    } => true,
                    _ => false,
                } =>
            {
                SwapBuffersError::TemporaryFailure(Box::new(Error::<E>::FramebufferCreationFailed(x)))
            }
            Error::Underlying(x) => x.into(),
            x => SwapBuffersError::ContextLost(Box::new(x)),
        }
    }
}
