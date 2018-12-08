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

use super::{Device, DeviceHandler, RawDevice, ResourceHandles, ResourceInfo, Surface};

use drm::control::{crtc, Device as ControlDevice};
use gbm::{self, BufferObjectFlags, Format as GbmFormat};
use nix::libc::dev_t;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use std::sync::{Once, ONCE_INIT};

pub mod error;
use self::error::*;

mod surface;
pub use self::surface::GbmSurface;
use self::surface::GbmSurfaceInternal;

#[cfg(feature = "backend_egl")]
pub mod egl;

#[cfg(feature = "backend_session")]
pub mod session;

static LOAD: Once = ONCE_INIT;

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
    pub fn new<L>(mut dev: D, logger: L) -> Result<Self>
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

        let log = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_gbm"));

        dev.clear_handler();

        debug!(log, "Creating gbm device");
        Ok(GbmDevice {
            // Open the gbm device from the drm device
            dev: Rc::new(RefCell::new(
                gbm::Device::new(dev).chain_err(|| ErrorKind::InitFailed)?,
            )),
            backends: Rc::new(RefCell::new(HashMap::new())),
            logger: log,
        })
    }
}

struct InternalDeviceHandler<D: RawDevice + ControlDevice + 'static> {
    handler: Box<DeviceHandler<Device = GbmDevice<D>> + 'static>,
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
        self.handler
            .error(ResultExt::<()>::chain_err(Err(error), || ErrorKind::UnderlyingBackendError).unwrap_err())
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

    fn create_surface(&mut self, crtc: crtc::Handle) -> Result<GbmSurface<D>> {
        info!(self.logger, "Initializing GbmSurface");

        let drm_surface = Device::create_surface(&mut **self.dev.borrow_mut(), crtc)
            .chain_err(|| ErrorKind::UnderlyingBackendError)?;

        // initialize the surface
        let (w, h) = drm_surface
            .pending_mode()
            .map(|mode| mode.size())
            .unwrap_or((1, 1));
        let surface = self
            .dev
            .borrow()
            .create_surface(
                w as u32,
                h as u32,
                GbmFormat::XRGB8888,
                BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING,
            )
            .chain_err(|| ErrorKind::SurfaceCreationFailed)?;

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
                .chain_err(|| ErrorKind::BufferCreationFailed)?,
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

    fn resource_info<T: ResourceInfo>(&self, handle: T::Handle) -> Result<T> {
        self.dev
            .borrow()
            .resource_info(handle)
            .chain_err(|| ErrorKind::UnderlyingBackendError)
    }

    fn resource_handles(&self) -> Result<ResourceHandles> {
        self.dev
            .borrow()
            .resource_handles()
            .chain_err(|| ErrorKind::UnderlyingBackendError)
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
