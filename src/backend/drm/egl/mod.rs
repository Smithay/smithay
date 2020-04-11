//!
//! [`Device`](Device) and [`Surface`](Surface)
//! implementations using egl contexts and surfaces for efficient rendering.
//!
//! Usually this implementation's [`EglSurface`](::backend::drm::egl::EglSurface)s implementation
//! of [`GLGraphicsBackend`](::backend::graphics::gl::GLGraphicsBackend) will be used
//! to let your compositor render.
//! Take a look at `anvil`s source code for an example of this.
//!

use drm::control::{crtc, connector, encoder, framebuffer, plane, ResourceHandles};
use drm::SystemError as DrmError;
use nix::libc::dev_t;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;
#[cfg(feature = "use_system_lib")]
use wayland_server::Display;

use super::{Device, DeviceHandler, Surface};
use crate::backend::egl::context::GlAttributes;
use crate::backend::egl::error::Result as EGLResult;
use crate::backend::egl::native::{Backend, NativeDisplay, NativeSurface};
use crate::backend::egl::EGLContext;
#[cfg(feature = "use_system_lib")]
use crate::backend::egl::{EGLDisplay, EGLGraphicsBackend};

pub mod error;
use self::error::*;

mod surface;
pub use self::surface::*;

#[cfg(feature = "backend_session")]
pub mod session;

/// Representation of an egl device to create egl rendering surfaces
pub struct EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B, Arguments = crtc::Handle> + 'static,
    <D as Device>::Surface: NativeSurface,
{
    dev: Rc<EGLContext<B, D>>,
    logger: ::slog::Logger,
}

impl<B, D> AsRawFd for EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B, Arguments = crtc::Handle> + 'static,
    <D as Device>::Surface: NativeSurface,
{
    fn as_raw_fd(&self) -> RawFd {
        self.dev.borrow().as_raw_fd()
    }
}

impl<B, D> EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B, Arguments = crtc::Handle> + 'static,
    <D as Device>::Surface: NativeSurface,
{
    /// Try to create a new [`EglDevice`] from an open device.
    ///
    /// Returns an error if the file is no valid device or context
    /// creation was not successful.
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

    /// Create a new [`EglDevice`] from an open device and given [`GlAttributes`]
    ///
    /// Returns an error if the file is no valid device or context
    /// creation was not successful.
    pub fn new_with_gl_attr<L>(mut dev: D, attributes: GlAttributes, logger: L) -> Result<Self>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_egl"));

        dev.clear_handler();

        debug!(log, "Creating egl context from device");
        Ok(EglDevice {
            // Open the gbm device from the drm device and create a context based on that
            dev: Rc::new(
                EGLContext::new(dev, attributes, Default::default(), log.clone()).map_err(Error::from)?,
            ),
            logger: log,
        })
    }
}

struct InternalDeviceHandler<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B, Arguments = crtc::Handle> + 'static,
    <D as Device>::Surface: NativeSurface,
{
    handler: Box<dyn DeviceHandler<Device = EglDevice<B, D>> + 'static>,
}

impl<B, D> DeviceHandler for InternalDeviceHandler<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B, Arguments = crtc::Handle> + 'static,
    <D as Device>::Surface: NativeSurface,
{
    type Device = D;

    fn vblank(&mut self, crtc: crtc::Handle) {
        self.handler.vblank(crtc)
    }
    fn error(&mut self, error: <<D as Device>::Surface as Surface>::Error) {
        self.handler
            .error(ResultExt::<()>::chain_err(Err(error), || ErrorKind::UnderlyingBackendError).unwrap_err())
    }
}

impl<B, D> Device for EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B, Arguments = crtc::Handle> + 'static,
    <D as Device>::Surface: NativeSurface,
{
    type Surface = EglSurface<B, D>;

    fn device_id(&self) -> dev_t {
        self.dev.borrow().device_id()
    }

    fn set_handler(&mut self, handler: impl DeviceHandler<Device = Self> + 'static) {
        self.dev.borrow_mut().set_handler(InternalDeviceHandler {
            handler: Box::new(handler),
        });
    }

    fn clear_handler(&mut self) {
        self.dev.borrow_mut().clear_handler()
    }

    fn create_surface(&mut self, crtc: crtc::Handle) -> Result<EglSurface<B, D>> {
        info!(self.logger, "Initializing EglSurface");

        let surface = self.dev.create_surface(crtc)?;

        Ok(EglSurface {
            dev: self.dev.clone(),
            surface,
        })
    }

    fn process_events(&mut self) {
        self.dev.borrow_mut().process_events()
    }

    fn resource_handles(&self) -> Result<ResourceHandles> {
        self.dev
            .borrow()
            .resource_handles()
            .chain_err(|| ErrorKind::UnderlyingBackendError)
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
    fn get_framebuffer_info(&self, fb: framebuffer::Handle) -> std::result::Result<framebuffer::Info, DrmError> {
        self.dev.borrow().get_framebuffer_info(fb)
    }
    fn get_plane_info(&self, plane: plane::Handle) -> std::result::Result<plane::Info, DrmError> {
        self.dev.borrow().get_plane_info(plane)
    }
}

#[cfg(feature = "use_system_lib")]
impl<B, D> EGLGraphicsBackend for EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B, Arguments = crtc::Handle> + 'static,
    <D as Device>::Surface: NativeSurface,
{
    fn bind_wl_display(&self, display: &Display) -> EGLResult<EGLDisplay> {
        self.dev.bind_wl_display(display)
    }
}

impl<B, D> Drop for EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B, Arguments = crtc::Handle> + 'static,
    <D as Device>::Surface: NativeSurface,
{
    fn drop(&mut self) {
        self.clear_handler();
    }
}
