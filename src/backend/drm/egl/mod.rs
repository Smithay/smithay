//!
//! [`Device`](Device) and [`Surface`](Surface)
//! implementations using egl contexts and surfaces for efficient rendering.
//!
//! Usually this implementation's [`EglSurface`](::backend::drm::egl::EglSurface)s implementation
//! of [`GLGraphicsBackend`](::backend::graphics::gl::GLGraphicsBackend) will be used
//! to let your compositor render.
//! Take a look at `anvil`s source code for an example of this.
//!

use drm::control::{connector, crtc, encoder, framebuffer, plane, Mode, ResourceHandles};
use drm::SystemError as DrmError;
use nix::libc::dev_t;
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::{Rc, Weak};
#[cfg(feature = "use_system_lib")]
use wayland_server::Display;

use super::{Device, DeviceHandler, Surface};
use crate::backend::egl::native::{Backend, NativeDisplay, NativeSurface};
#[cfg(feature = "use_system_lib")]
use crate::backend::egl::{display::EGLBufferReader, EGLGraphicsBackend};
use crate::backend::egl::{EGLError as RawEGLError, Error as EGLError, SurfaceCreationError};

mod surface;
pub use self::surface::*;
use crate::backend::egl::context::{GlAttributes, PixelFormatRequirements};
use crate::backend::egl::display::EGLDisplay;

#[cfg(feature = "backend_session")]
pub mod session;

/// Errors for the DRM/EGL module
#[derive(thiserror::Error, Debug)]
pub enum Error<U: std::error::Error + std::fmt::Debug + std::fmt::Display + 'static> {
    /// EGL Error
    #[error("EGL error: {0:}")]
    EGL(#[source] EGLError),
    /// EGL Error
    #[error("EGL error: {0:}")]
    RawEGL(#[source] RawEGLError),
    /// Underlying backend error
    #[error("Underlying backend error: {0:?}")]
    Underlying(#[source] U),
}

type Arguments = (crtc::Handle, Mode, Vec<connector::Handle>);

/// Representation of an egl device to create egl rendering surfaces
pub struct EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device
        + NativeDisplay<B, Arguments = Arguments, Error = <<D as Device>::Surface as Surface>::Error>
        + 'static,
    <D as Device>::Surface: NativeSurface,
{
    dev: EGLDisplay<B, D>,
    logger: ::slog::Logger,
    default_attributes: GlAttributes,
    default_requirements: PixelFormatRequirements,
    backends: Rc<RefCell<HashMap<crtc::Handle, Weak<EglSurfaceInternal<<D as Device>::Surface>>>>>,
}

impl<B, D> AsRawFd for EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device
        + NativeDisplay<B, Arguments = Arguments, Error = <<D as Device>::Surface as Surface>::Error>
        + 'static,
    <D as Device>::Surface: NativeSurface,
{
    fn as_raw_fd(&self) -> RawFd {
        self.dev.borrow().as_raw_fd()
    }
}

impl<B, D> EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device
        + NativeDisplay<B, Arguments = Arguments, Error = <<D as Device>::Surface as Surface>::Error>
        + 'static,
    <D as Device>::Surface: NativeSurface,
{
    /// Try to create a new [`EglDevice`] from an open device.
    ///
    /// Returns an error if the file is no valid device or context
    /// creation was not successful.
    pub fn new<L>(dev: D, logger: L) -> Result<Self, Error<<<D as Device>::Surface as Surface>::Error>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        EglDevice::new_with_defaults(
            dev,
            GlAttributes {
                version: None,
                profile: None,
                debug: cfg!(debug_assertions),
                vsync: true,
            },
            Default::default(),
            logger,
        )
    }

    /// Try to create a new [`EglDevice`] from an open device with the given attributes and
    /// requirements as defaults for new surfaces.
    ///
    /// Returns an error if the file is no valid device or context
    /// creation was not successful.
    pub fn new_with_defaults<L>(
        mut dev: D,
        default_attributes: GlAttributes,
        default_requirements: PixelFormatRequirements,
        logger: L,
    ) -> Result<Self, Error<<<D as Device>::Surface as Surface>::Error>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_egl"));

        dev.clear_handler();

        debug!(log, "Creating egl context from device");
        Ok(EglDevice {
            dev: EGLDisplay::new(dev, log.clone()).map_err(Error::EGL)?,
            default_attributes,
            default_requirements,
            backends: Rc::new(RefCell::new(HashMap::new())),
            logger: log,
        })
    }
}

struct InternalDeviceHandler<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device
        + NativeDisplay<B, Arguments = Arguments, Error = <<D as Device>::Surface as Surface>::Error>
        + 'static,
    <D as Device>::Surface: NativeSurface,
{
    handler: Box<dyn DeviceHandler<Device = EglDevice<B, D>> + 'static>,
}

impl<B, D> DeviceHandler for InternalDeviceHandler<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device
        + NativeDisplay<B, Arguments = Arguments, Error = <<D as Device>::Surface as Surface>::Error>
        + 'static,
    <D as Device>::Surface: NativeSurface,
{
    type Device = D;

    fn vblank(&mut self, crtc: crtc::Handle) {
        self.handler.vblank(crtc)
    }
    fn error(&mut self, error: <<D as Device>::Surface as Surface>::Error) {
        self.handler.error(Error::Underlying(error));
    }
}

impl<B, D> Device for EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device
        + NativeDisplay<B, Arguments = Arguments, Error = <<D as Device>::Surface as Surface>::Error>
        + 'static,
    <D as Device>::Surface: NativeSurface,
{
    type Surface = EglSurface<<D as Device>::Surface>;

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

    fn create_surface(
        &mut self,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
    ) -> Result<Self::Surface, <Self::Surface as Surface>::Error> {
        info!(self.logger, "Initializing EglSurface");

        let context = self
            .dev
            .create_context(self.default_attributes, self.default_requirements)
            .map_err(Error::EGL)?;
        let surface = self
            .dev
            .create_surface(
                context.get_pixel_format(),
                self.default_requirements.double_buffer,
                context.get_config_id(),
                (crtc, mode, Vec::from(connectors)),
            )
            .map_err(|err| match err {
                SurfaceCreationError::EGLSurfaceCreationFailed(err) => Error::RawEGL(err),
                SurfaceCreationError::NativeSurfaceCreationFailed(err) => Error::Underlying(err),
            })?;

        let backend = Rc::new(EglSurfaceInternal { context, surface });
        self.backends.borrow_mut().insert(crtc, Rc::downgrade(&backend));
        Ok(EglSurface(backend))
    }

    fn process_events(&mut self) {
        self.dev.borrow_mut().process_events()
    }

    fn resource_handles(&self) -> Result<ResourceHandles, <Self::Surface as Surface>::Error> {
        self.dev.borrow().resource_handles().map_err(Error::Underlying)
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

#[cfg(feature = "use_system_lib")]
impl<B, D> EGLGraphicsBackend for EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device
        + NativeDisplay<B, Arguments = Arguments, Error = <<D as Device>::Surface as Surface>::Error>
        + 'static,
    <D as Device>::Surface: NativeSurface,
{
    fn bind_wl_display(&self, display: &Display) -> Result<EGLBufferReader, EGLError> {
        self.dev.bind_wl_display(display)
    }
}

impl<B, D> Drop for EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device
        + NativeDisplay<B, Arguments = Arguments, Error = <<D as Device>::Surface as Surface>::Error>
        + 'static,
    <D as Device>::Surface: NativeSurface,
{
    fn drop(&mut self) {
        self.clear_handler();
    }
}
