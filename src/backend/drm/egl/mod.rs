use drm::control::{crtc, Mode};
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
use wayland_server::Display;

use super::{Device, DeviceHandler, Surface};
use backend::egl::context::GlAttributes;
use backend::egl::error::Result as EGLResult;
use backend::egl::native::{Backend, NativeDisplay, NativeSurface};
use backend::egl::{EGLContext, EGLDisplay, EGLGraphicsBackend};

pub mod error;
use self::error::*;

mod surface;
pub use self::surface::*;

#[cfg(feature = "backend_session")]
pub mod session;

/// Representation of an open gbm device to create rendering backends
pub struct EglDevice<
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B> + 'static,
> where
    <D as Device>::Surface: NativeSurface,
{
    dev: Rc<RefCell<EGLContext<B, D>>>,
    backends: Rc<RefCell<HashMap<crtc::Handle, Weak<EglSurface<B, D>>>>>,
    logger: ::slog::Logger,
}

impl<B: Backend<Surface = <D as Device>::Surface> + 'static, D: Device + NativeDisplay<B> + 'static> AsRawFd
    for EglDevice<B, D>
where
    <D as Device>::Surface: NativeSurface,
{
    fn as_raw_fd(&self) -> RawFd {
        self.dev.borrow().as_raw_fd()
    }
}

impl<B: Backend<Surface = <D as Device>::Surface> + 'static, D: Device + NativeDisplay<B> + 'static>
    EglDevice<B, D>
where
    <D as Device>::Surface: NativeSurface,
{
    /// Create a new `EglGbmDrmDevice` from an open drm node
    ///
    /// Returns an error if the file is no valid drm node or context creation was not
    /// successful.
    pub fn new<L>(dev: D, logger: L) -> Result<Self>
    where
        L: Into<Option<::slog::Logger>>,
    {
        EglDevice::new_with_gl_attr(
            dev,
            GlAttributes {
                version: None,
                profile: None,
                debug: cfg!(debug_assertions),
                vsync: true,
            },
            logger,
        )
    }

    /// Create a new `EglGbmDrmDevice` from an open `RawDevice` and given `GlAttributes`
    ///
    /// Returns an error if the file is no valid drm node or context creation was not
    /// successful.
    pub fn new_with_gl_attr<L>(mut dev: D, attributes: GlAttributes, logger: L) -> Result<Self>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_egl"));

        dev.clear_handler();

        debug!(log, "Creating egl context from device");
        Ok(EglDevice {
            // Open the gbm device from the drm device and create a context based on that
            dev: Rc::new(RefCell::new(
                EGLContext::new(dev, attributes, Default::default(), log.clone()).map_err(Error::from)?,
            )),
            backends: Rc::new(RefCell::new(HashMap::new())),
            logger: log,
        })
    }
}

struct InternalDeviceHandler<
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B> + 'static,
> where
    <D as Device>::Surface: NativeSurface,
{
    handler: Box<DeviceHandler<Device = EglDevice<B, D>> + 'static>,
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<EglSurface<B, D>>>>>,
    logger: ::slog::Logger,
}

impl<B: Backend<Surface = <D as Device>::Surface> + 'static, D: Device + NativeDisplay<B> + 'static>
    DeviceHandler for InternalDeviceHandler<B, D>
where
    <D as NativeDisplay<B>>::Arguments: From<(
        crtc::Handle,
        Mode,
        <<D as Device>::Surface as Surface>::Connectors,
    )>,
    <D as Device>::Surface: NativeSurface,
{
    type Device = D;

    fn vblank(&mut self, surface: &<D as Device>::Surface) {
        if let Some(backends) = self.backends.upgrade() {
            if let Some(surface) = backends.borrow().get(&surface.crtc()) {
                if let Some(surface) = surface.upgrade() {
                    self.handler.vblank(&*surface);
                }
            } else {
                warn!(
                    self.logger,
                    "Surface ({:?}) not managed by egl, event not handled.",
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

impl<B: Backend<Surface = <D as Device>::Surface> + 'static, D: Device + NativeDisplay<B> + 'static> Device
    for EglDevice<B, D>
where
    <D as NativeDisplay<B>>::Arguments: From<(
        crtc::Handle,
        Mode,
        <<D as Device>::Surface as Surface>::Connectors,
    )>,
    <D as Device>::Surface: NativeSurface,
{
    type Surface = EglSurface<B, D>;
    type Return = Rc<EglSurface<B, D>>;

    fn set_handler(&mut self, handler: impl DeviceHandler<Device = Self> + 'static) {
        self.dev.borrow_mut().set_handler(InternalDeviceHandler {
            handler: Box::new(handler),
            backends: Rc::downgrade(&self.backends),
            logger: self.logger.clone(),
        });
    }

    fn clear_handler(&mut self) {
        self.dev.borrow_mut().clear_handler()
    }

    fn create_surface(
        &mut self,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: impl Into<<<Self as Device>::Surface as Surface>::Connectors>,
    ) -> Result<Rc<EglSurface<B, D>>> {
        info!(self.logger, "Initializing EglSurface");

        let surface = self
            .dev
            .borrow_mut()
            .create_surface((crtc, mode, connectors.into()).into())?;

        let backend = Rc::new(EglSurface {
            dev: self.dev.clone(),
            surface,
        });
        self.backends.borrow_mut().insert(crtc, Rc::downgrade(&backend));
        Ok(backend)
    }

    fn process_events(&mut self) {
        self.dev.borrow_mut().process_events()
    }
}

impl<B: Backend<Surface = <D as Device>::Surface> + 'static, D: Device + NativeDisplay<B> + 'static>
    EGLGraphicsBackend for EglDevice<B, D>
where
    <D as Device>::Surface: NativeSurface,
{
    fn bind_wl_display(&self, display: &Display) -> EGLResult<EGLDisplay> {
        self.dev.borrow().bind_wl_display(display)
    }
}
