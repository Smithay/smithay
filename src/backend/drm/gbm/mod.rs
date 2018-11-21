use super::{Device, DeviceHandler, RawDevice, Surface};

use drm::control::{crtc, framebuffer, Device as ControlDevice, Mode};
use gbm::{self, BufferObjectFlags, Format as GbmFormat};

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use std::sync::{Once, ONCE_INIT};

pub mod error;
use self::error::*;

mod surface;
pub use self::surface::GbmSurface;

pub mod egl;

#[cfg(feature = "backend_session")]
pub mod session;

static LOAD: Once = ONCE_INIT;

/// Representation of an open gbm device to create rendering backends
pub struct GbmDevice<D: RawDevice + ControlDevice + 'static>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>,
{
    pub(self) dev: Rc<RefCell<gbm::Device<D>>>,
    backends: Rc<RefCell<HashMap<crtc::Handle, Weak<GbmSurface<D>>>>>,
    logger: ::slog::Logger,
}

impl<D: RawDevice + ControlDevice + 'static> GbmDevice<D>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>,
{
    /// Create a new `GbmDevice` from an open drm node
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

struct InternalDeviceHandler<D: RawDevice + ControlDevice + 'static>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>,
{
    handler: Box<DeviceHandler<Device = GbmDevice<D>> + 'static>,
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<GbmSurface<D>>>>>,
    logger: ::slog::Logger,
}

impl<D: RawDevice + ControlDevice + 'static> DeviceHandler for InternalDeviceHandler<D>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>,
{
    type Device = D;

    fn vblank(&mut self, surface: &<D as Device>::Surface) {
        if let Some(backends) = self.backends.upgrade() {
            if let Some(surface) = backends.borrow().get(&surface.crtc()) {
                if let Some(surface) = surface.upgrade() {
                    surface.unlock_buffer();
                    self.handler.vblank(&*surface);
                }
            } else {
                warn!(
                    self.logger,
                    "Surface ({:?}) not managed by gbm, event not handled.",
                    surface.crtc()
                );
            }
        }
    }
    fn error(&mut self, error: <<D as Device>::Surface as Surface>::Error) {
        self.handler
            .error(ResultExt::<()>::chain_err(Err(error), || ErrorKind::UnderlyingBackendError).unwrap_err())
    }
}

impl<D: RawDevice + ControlDevice + 'static> Device for GbmDevice<D>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>,
{
    type Surface = GbmSurface<D>;
    type Return = Rc<GbmSurface<D>>;

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
        connectors: impl Into<<Self::Surface as Surface>::Connectors>,
    ) -> Result<Rc<GbmSurface<D>>> {
        info!(self.logger, "Initializing GbmSurface");

        let (w, h) = mode.size();
        let surface = self
            .dev
            .borrow()
            .create_surface(
                w as u32,
                h as u32,
                GbmFormat::XRGB8888,
                BufferObjectFlags::SCANOUT | BufferObjectFlags::RENDERING,
            ).chain_err(|| ErrorKind::SurfaceCreationFailed)?;

        // init the first screen
        // (must be done before calling page_flip for the first time)
        let mut front_bo = surface
            .lock_front_buffer()
            .chain_err(|| ErrorKind::FrontBufferLockFailed)?;

        debug!(self.logger, "FrontBuffer color format: {:?}", front_bo.format());

        // we need a framebuffer for the front buffer
        let fb = framebuffer::create(&*self.dev.borrow(), &*front_bo)
            .chain_err(|| ErrorKind::UnderlyingBackendError)?;
        front_bo.set_userdata(fb).unwrap();

        let cursor = Cell::new((
            self.dev
                .borrow()
                .create_buffer_object(
                    1,
                    1,
                    GbmFormat::ARGB8888,
                    BufferObjectFlags::CURSOR | BufferObjectFlags::WRITE,
                ).chain_err(|| ErrorKind::BufferCreationFailed)?,
            (0, 0),
        ));

        let backend = Rc::new(GbmSurface {
            dev: self.dev.clone(),
            surface: RefCell::new(surface),
            crtc: Device::create_surface(&mut **self.dev.borrow_mut(), crtc, mode, connectors)
                .chain_err(|| ErrorKind::UnderlyingBackendError)?,
            cursor,
            current_frame_buffer: Cell::new(fb),
            front_buffer: Cell::new(front_bo),
            next_buffer: Cell::new(None),
            logger: self.logger.new(o!("crtc" => format!("{:?}", crtc))),
        });
        self.backends.borrow_mut().insert(crtc, Rc::downgrade(&backend));
        Ok(backend)
    }

    fn process_events(&mut self) {
        self.dev.borrow_mut().process_events()
    }
}

impl<D: RawDevice + ControlDevice + 'static> AsRawFd for GbmDevice<D>
where
    <D as Device>::Return: ::std::borrow::Borrow<<D as RawDevice>::Surface>,
{
    fn as_raw_fd(&self) -> RawFd {
        self.dev.borrow().as_raw_fd()
    }
}
