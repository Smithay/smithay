//!
//! Types to make fallback device initialization easier
//!

#[cfg(feature = "backend_drm_egl")]
use crate::backend::drm::egl::{Arguments as EglDeviceArguments, EglDevice, Error as EglDeviceError};
#[cfg(all(feature = "backend_drm_atomic", feature = "backend_drm_legacy"))]
use crate::backend::drm::{atomic::AtomicDrmDevice, legacy::LegacyDrmDevice};
use crate::backend::drm::{common::Error, Device, DeviceHandler, RawDevice, RawSurface, Surface};
#[cfg(all(feature = "backend_drm_gbm", feature = "backend_drm_eglstream"))]
use crate::backend::drm::{
    eglstream::{EglStreamDevice, Error as EglStreamError},
    gbm::{Error as GbmError, GbmDevice},
};
#[cfg(feature = "backend_drm_egl")]
use crate::backend::egl::context::{GlAttributes, PixelFormatRequirements};
#[cfg(feature = "backend_drm_egl")]
use crate::backend::egl::native::{Backend, NativeDisplay, NativeSurface};
use crate::backend::egl::Error as EGLError;
#[cfg(feature = "use_system_lib")]
use crate::backend::egl::{display::EGLBufferReader, EGLGraphicsBackend};
#[cfg(feature = "renderer_gl")]
use crate::backend::graphics::gl::GLGraphicsBackend;
#[cfg(feature = "renderer_gl")]
use crate::backend::graphics::PixelFormat;
use crate::backend::graphics::{CursorBackend, SwapBuffersError};

use drm::{
    control::{connector, crtc, encoder, framebuffer, plane, Device as ControlDevice, Mode, ResourceHandles},
    Device as BasicDevice, SystemError as DrmError,
};
#[cfg(feature = "renderer_gl")]
use nix::libc::c_void;
use nix::libc::dev_t;
use std::env;
use std::os::unix::io::{AsRawFd, RawFd};
#[cfg(feature = "use_system_lib")]
use wayland_server::Display;

/// [`Device`](::backend::drm::Device) Wrapper to assist fallback
/// in case initialization of the preferred device type fails.
pub enum FallbackDevice<D1: Device + 'static, D2: Device + 'static> {
    /// Variant for successful initialization of the preferred device
    Preference(D1),
    /// Variant for the fallback device
    Fallback(D2),
}

struct FallbackDeviceHandlerD1<E1, E2, C, S1, S2, D1, D2>(
    Box<dyn DeviceHandler<Device = FallbackDevice<D1, D2>> + 'static>,
)
where
    E1: std::error::Error + Send + 'static,
    E2: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E1, Connectors = C> + 'static,
    S2: Surface<Error = E2, Connectors = C> + 'static,
    D1: Device<Surface = S1> + 'static,
    D2: Device<Surface = S2> + 'static;

impl<E1, E2, C, S1, S2, D1, D2> DeviceHandler for FallbackDeviceHandlerD1<E1, E2, C, S1, S2, D1, D2>
where
    E1: std::error::Error + Send + 'static,
    E2: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E1, Connectors = C> + 'static,
    S2: Surface<Error = E2, Connectors = C> + 'static,
    D1: Device<Surface = S1> + 'static,
    D2: Device<Surface = S2> + 'static,
{
    type Device = D1;

    fn vblank(&mut self, crtc: crtc::Handle) {
        self.0.vblank(crtc)
    }
    fn error(&mut self, error: E1) {
        self.0.error(EitherError::Either(error));
    }
}

struct FallbackDeviceHandlerD2<E1, E2, C, S1, S2, D1, D2>(
    Box<dyn DeviceHandler<Device = FallbackDevice<D1, D2>> + 'static>,
)
where
    E1: std::error::Error + Send + 'static,
    E2: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E1, Connectors = C> + 'static,
    S2: Surface<Error = E2, Connectors = C> + 'static,
    D1: Device<Surface = S1> + 'static,
    D2: Device<Surface = S2> + 'static;

impl<E1, E2, C, S1, S2, D1, D2> DeviceHandler for FallbackDeviceHandlerD2<E1, E2, C, S1, S2, D1, D2>
where
    E1: std::error::Error + Send + 'static,
    E2: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E1, Connectors = C> + 'static,
    S2: Surface<Error = E2, Connectors = C> + 'static,
    D1: Device<Surface = S1> + 'static,
    D2: Device<Surface = S2> + 'static,
{
    type Device = D2;

    fn vblank(&mut self, crtc: crtc::Handle) {
        self.0.vblank(crtc)
    }
    fn error(&mut self, error: E2) {
        self.0.error(EitherError::Or(error));
    }
}

#[cfg(feature = "backend_session")]
impl<D1, D2> crate::signaling::Linkable<crate::backend::session::Signal> for FallbackDevice<D1, D2>
where
    D1: Device + crate::signaling::Linkable<crate::backend::session::Signal> + 'static,
    D2: Device + crate::signaling::Linkable<crate::backend::session::Signal> + 'static,
{
    fn link(&mut self, signal: crate::signaling::Signaler<crate::backend::session::Signal>) {
        match self {
            FallbackDevice::Preference(d) => d.link(signal),
            FallbackDevice::Fallback(d) => d.link(signal),
        }
    }
}

/// [`Surface`](::backend::drm::Surface) Wrapper to assist fallback
/// in case initialization of the preferred device type fails.
pub enum FallbackSurface<S1: Surface, S2: Surface> {
    /// Variant for successful initialization of the preferred device
    Preference(S1),
    /// Variant for the fallback device
    Fallback(S2),
}

/// Enum uniting two kinds of possible errors.
#[derive(Debug, thiserror::Error)]
pub enum EitherError<E1: std::error::Error + 'static, E2: std::error::Error + 'static> {
    /// Either this error
    #[error("{0}")]
    Either(#[source] E1),
    /// Or this error
    #[error("{0}")]
    Or(#[source] E2),
}

impl<E1, E2> Into<SwapBuffersError> for EitherError<E1, E2>
where
    E1: std::error::Error + Into<SwapBuffersError> + 'static,
    E2: std::error::Error + Into<SwapBuffersError> + 'static,
{
    fn into(self) -> SwapBuffersError {
        match self {
            EitherError::Either(err) => err.into(),
            EitherError::Or(err) => err.into(),
        }
    }
}

#[cfg(all(feature = "backend_drm_atomic", feature = "backend_drm_legacy"))]
impl<A: AsRawFd + Clone + 'static> FallbackDevice<AtomicDrmDevice<A>, LegacyDrmDevice<A>> {
    /// Try to initialize an [`AtomicDrmDevice`](::backend::drm::atomic::AtomicDrmDevice)
    /// and fall back to a [`LegacyDrmDevice`] if atomic-modesetting is not supported.
    ///
    /// # Arguments
    ///
    /// - `fd` - Open drm node (needs to be clonable to be passed to multiple initializers)
    /// - `disable_connectors` - Setting this to true will initialize all connectors \
    ///     as disabled on device creation. smithay enables connectors, when attached \
    ///     to a surface, and disables them, when detached. Setting this to `false` \
    ///     requires usage of `drm-rs` to disable unused connectors to prevent them \
    ///     showing garbage, but will also prevent flickering of already turned on \
    ///     connectors (assuming you won't change the resolution).
    /// - `logger` - Optional [`slog::Logger`] to be used by the resulting device.
    ///
    /// # Return
    ///
    /// Returns an error, if both devices fail to initialize due to `fd` being no valid
    /// drm node or the device being not accessible.
    pub fn new<L>(fd: A, disable_connectors: bool, logger: L) -> Result<Self, Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_drm_fallback"));
        info!(log, "Trying to initialize AtomicDrmDevice");

        let force_legacy = env::var("SMITHAY_USE_LEGACY")
            .map(|x| {
                x == "1" || x.to_lowercase() == "true" || x.to_lowercase() == "yes" || x.to_lowercase() == "y"
            })
            .unwrap_or(false);
        if force_legacy {
            info!(log, "SMITHAY_USE_LEGACY is set. Forcing LegacyDrmDevice.");
            return Ok(FallbackDevice::Fallback(LegacyDrmDevice::new(
                fd,
                disable_connectors,
                log,
            )?));
        }

        match AtomicDrmDevice::new(fd.clone(), disable_connectors, log.clone()) {
            Ok(dev) => Ok(FallbackDevice::Preference(dev)),
            Err(err) => {
                warn!(log, "Failed to initialize preferred AtomicDrmDevice: {}", err);
                info!(log, "Falling back to fallback LegacyDrmDevice");
                Ok(FallbackDevice::Fallback(LegacyDrmDevice::new(
                    fd,
                    disable_connectors,
                    log,
                )?))
            }
        }
    }
}

#[cfg(all(
    feature = "backend_drm_gbm",
    feature = "backend_drm_eglstream",
    feature = "backend_udev"
))]
type GbmOrEglStreamError<D> = EitherError<
    GbmError<<<D as Device>::Surface as Surface>::Error>,
    EglStreamError<<<D as Device>::Surface as Surface>::Error>,
>;
#[cfg(all(
    feature = "backend_drm_gbm",
    feature = "backend_drm_eglstream",
    feature = "backend_udev"
))]
impl<D> FallbackDevice<GbmDevice<D>, EglStreamDevice<D>>
where
    D: RawDevice + ControlDevice + 'static,
{
    /// Try to initialize a [`GbmDevice`](::backend::drm::gbm::GbmDevice)
    /// or a [`EglStreamDevice`](::backend::drm::eglstream::EglStreamDevice) depending on the used driver.
    ///
    /// # Arguments
    ///
    /// - `dev` - Open drm device (needs implement [`RawDevice`](::backend::drm::RawDevice))
    /// - `logger` - Optional [`slog::Logger`] to be used by the resulting device.
    ///
    /// # Return
    ///
    /// Returns an error, if the choosen device fails to initialize.
    pub fn new<L>(dev: D, logger: L) -> Result<Self, GbmOrEglStreamError<D>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_drm_fallback"));

        let driver = crate::backend::udev::driver(dev.device_id()).expect("Failed to query device");
        info!(log, "Drm device driver: {:?}", driver);
        if driver.as_ref().and_then(|x| x.to_str()) == Some("nvidia") {
            Ok(FallbackDevice::Fallback(
                EglStreamDevice::new(dev, log).map_err(EitherError::Or)?,
            ))
        } else {
            Ok(FallbackDevice::Preference(
                GbmDevice::new(dev, log.clone()).map_err(EitherError::Either)?,
            ))
        }
    }
}

#[cfg(feature = "backend_drm_egl")]
type EglUnderlying<D1, D2> = EitherError<
    EglDeviceError<<<D1 as Device>::Surface as Surface>::Error>,
    EglDeviceError<<<D2 as Device>::Surface as Surface>::Error>,
>;

#[cfg(feature = "backend_drm_egl")]
type FallbackEglDevice<B1, D1, B2, D2> = FallbackDevice<EglDevice<B1, D1>, EglDevice<B2, D2>>;

#[cfg(feature = "backend_drm_egl")]
impl<D1, D2> FallbackDevice<D1, D2>
where
    D1: Device + 'static,
    <D1 as Device>::Surface: NativeSurface<Error = <<D1 as Device>::Surface as Surface>::Error>,
    D2: Device + 'static,
    <D2 as Device>::Surface: NativeSurface<Error = <<D2 as Device>::Surface as Surface>::Error>,
{
    /// Try to create a new [`EglDevice`] from a [`FallbackDevice`] containing two compatible device types.
    ///
    /// This helper function is necessary as implementing [`NativeDevice`](::backend::egl::native::NativeDevice) for [`FallbackDevice`] is impossible
    /// as selecting the appropriate [`Backend`](::backend::egl::native::Backend) would be impossible without knowing
    /// the underlying device type, that was selected by [`FallbackDevice`].
    ///
    /// Returns an error if the context creation was not successful.
    pub fn new_egl<B1, B2, L>(
        dev: FallbackDevice<D1, D2>,
        logger: L,
    ) -> Result<FallbackEglDevice<B1, D1, B2, D2>, EglUnderlying<D1, D2>>
    where
        B1: Backend<Surface = <D1 as Device>::Surface, Error = <<D1 as Device>::Surface as Surface>::Error>
            + 'static,
        D1: NativeDisplay<B1, Arguments = EglDeviceArguments>,
        B2: Backend<Surface = <D2 as Device>::Surface, Error = <<D2 as Device>::Surface as Surface>::Error>
            + 'static,
        D2: NativeDisplay<B2, Arguments = EglDeviceArguments>,
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_drm_fallback"));
        match dev {
            FallbackDevice::Preference(gbm) => match EglDevice::new(gbm, log) {
                Ok(dev) => Ok(FallbackDevice::Preference(dev)),
                Err(err) => Err(EglUnderlying::<D1, D2>::Either(err)),
            },
            FallbackDevice::Fallback(eglstream) => match EglDevice::new(eglstream, log) {
                Ok(dev) => Ok(FallbackDevice::Fallback(dev)),
                Err(err) => Err(EglUnderlying::<D1, D2>::Or(err)),
            },
        }
    }

    /// Try to create a new [`EglDevice`] from a [`FallbackDevice`] containing two compatible device types with
    /// the given attributes and requirements as defaults for new surfaces.
    ///
    /// This helper function is necessary as implementing [`NativeDevice`](::backend::egl::native::NativeDevice) for [`FallbackDevice`] is impossible
    /// as selecting the appropriate [`Backend`](::backend::egl::native::Backend) would be impossible without knowing
    /// the underlying device type, that was selected by [`FallbackDevice`].
    ///
    /// Returns an error if the context creation was not successful.
    pub fn new_egl_with_defaults<B1, B2, L>(
        dev: FallbackDevice<D1, D2>,
        default_attributes: GlAttributes,
        default_requirements: PixelFormatRequirements,
        logger: L,
    ) -> Result<FallbackEglDevice<B1, D1, B2, D2>, EglUnderlying<D1, D2>>
    where
        B1: Backend<Surface = <D1 as Device>::Surface, Error = <<D1 as Device>::Surface as Surface>::Error>
            + 'static,
        D1: NativeDisplay<B1, Arguments = EglDeviceArguments>,
        B2: Backend<Surface = <D2 as Device>::Surface, Error = <<D2 as Device>::Surface as Surface>::Error>
            + 'static,
        D2: NativeDisplay<B2, Arguments = EglDeviceArguments>,
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_drm_fallback"));
        match dev {
            FallbackDevice::Preference(gbm) => {
                match EglDevice::new_with_defaults(gbm, default_attributes, default_requirements, log) {
                    Ok(dev) => Ok(FallbackDevice::Preference(dev)),
                    Err(err) => Err(EglUnderlying::<D1, D2>::Either(err)),
                }
            }
            FallbackDevice::Fallback(eglstream) => {
                match EglDevice::new_with_defaults(eglstream, default_attributes, default_requirements, log) {
                    Ok(dev) => Ok(FallbackDevice::Fallback(dev)),
                    Err(err) => Err(EglUnderlying::<D1, D2>::Or(err)),
                }
            }
        }
    }
}

macro_rules! fallback_device_impl {
    ($func_name:ident, $self:ty, $return:ty, $($arg_name:ident : $arg_ty:ty),*) => {
        fn $func_name(self: $self, $($arg_name : $arg_ty),*) -> $return {
            match self {
                FallbackDevice::Preference(dev) => dev.$func_name($($arg_name),*),
                FallbackDevice::Fallback(dev) => dev.$func_name($($arg_name),*),
            }
        }
    };
    ($func_name:ident, $self:ty, $return:ty) => {
        fallback_device_impl!($func_name, $self, $return,);
    };
    ($func_name:ident, $self:ty) => {
        fallback_device_impl!($func_name, $self, ());
    };
}
macro_rules! fallback_device_err_impl {
    ($func_name:ident, $self:ty, $return:ty, $($arg_name:ident : $arg_ty:ty),*) => {
        fn $func_name(self: $self, $($arg_name : $arg_ty),*) -> $return {
            match self {
                FallbackDevice::Preference(dev) => dev.$func_name($($arg_name),*).map_err(EitherError::Either),
                FallbackDevice::Fallback(dev) => dev.$func_name($($arg_name),*).map_err(EitherError::Or),
            }
        }
    };
    ($func_name:ident, $self:ty, $return:ty) => {
        fallback_device_err_impl!($func_name, $self, $return,);
    };
}

macro_rules! fallback_surface_impl {
    ($func_name:ident, $self:ty, $return:ty, $($arg_name:ident : $arg_ty:ty),*) => {
        fn $func_name(self: $self, $($arg_name : $arg_ty),*) -> $return {
            match self {
                FallbackSurface::Preference(dev) => dev.$func_name($($arg_name),*),
                FallbackSurface::Fallback(dev) => dev.$func_name($($arg_name),*),
            }
        }
    };
    ($func_name:ident, $self:ty, $return:ty) => {
        fallback_surface_impl!($func_name, $self, $return,);
    };
    ($func_name:ident, $self:ty) => {
        fallback_surface_impl!($func_name, $self, ());
    };
}
macro_rules! fallback_surface_err_impl {
    ($func_name:ident, $self:ty, $return:ty, $($arg_name:ident : $arg_ty:ty),*) => {
        fn $func_name(self: $self, $($arg_name : $arg_ty),*) -> $return {
            match self {
                FallbackSurface::Preference(dev) => dev.$func_name($($arg_name),*).map_err(EitherError::Either),
                FallbackSurface::Fallback(dev) => dev.$func_name($($arg_name),*).map_err(EitherError::Or),
            }
        }
    };
}

impl<D1: Device, D2: Device> AsRawFd for FallbackDevice<D1, D2> {
    fallback_device_impl!(as_raw_fd, &Self, RawFd);
}
impl<D1: Device + BasicDevice, D2: Device + BasicDevice> BasicDevice for FallbackDevice<D1, D2> {}
impl<D1: Device + ControlDevice, D2: Device + ControlDevice> ControlDevice for FallbackDevice<D1, D2> {}

impl<E1, E2, C, S1, S2, D1, D2> Device for FallbackDevice<D1, D2>
where
    // Connectors need to match for both Surfaces
    E1: std::error::Error + Send + 'static,
    E2: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E1, Connectors = C> + 'static,
    S2: Surface<Error = E2, Connectors = C> + 'static,
    D1: Device<Surface = S1> + 'static,
    D2: Device<Surface = S2> + 'static,
{
    type Surface = FallbackSurface<S1, S2>;

    fallback_device_impl!(device_id, &Self, dev_t);
    fn set_handler(&mut self, handler: impl DeviceHandler<Device = Self> + 'static) {
        match self {
            FallbackDevice::Preference(dev) => dev.set_handler(FallbackDeviceHandlerD1(Box::new(handler))),
            FallbackDevice::Fallback(dev) => dev.set_handler(FallbackDeviceHandlerD2(Box::new(handler))),
        }
    }
    fallback_device_impl!(clear_handler, &mut Self);
    fn create_surface(
        &mut self,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
    ) -> Result<Self::Surface, EitherError<E1, E2>> {
        match self {
            FallbackDevice::Preference(dev) => Ok(FallbackSurface::Preference(
                dev.create_surface(crtc, mode, connectors)
                    .map_err(EitherError::Either)?,
            )),
            FallbackDevice::Fallback(dev) => Ok(FallbackSurface::Fallback(
                dev.create_surface(crtc, mode, connectors)
                    .map_err(EitherError::Or)?,
            )),
        }
    }
    fallback_device_impl!(process_events, &mut Self);
    fallback_device_err_impl!(resource_handles, &Self, Result<ResourceHandles, EitherError<E1, E2>>);
    fallback_device_impl!(get_connector_info, &Self, Result<connector::Info, DrmError>, conn: connector::Handle);
    fallback_device_impl!(get_crtc_info, &Self, Result<crtc::Info, DrmError>, crtc: crtc::Handle);
    fallback_device_impl!(get_encoder_info, &Self, Result<encoder::Info, DrmError>, enc: encoder::Handle);
    fallback_device_impl!(get_framebuffer_info, &Self, Result<framebuffer::Info, DrmError>, fb: framebuffer::Handle);
    fallback_device_impl!(get_plane_info, &Self, Result<plane::Info, DrmError>, plane : plane::Handle);
}

// Impl RawDevice where underlying types implement RawDevice
impl<E1, E2, C, S1, S2, D1, D2> RawDevice for FallbackDevice<D1, D2>
where
    // Connectors need to match for both Surfaces
    E1: std::error::Error + Send + 'static,
    E2: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: RawSurface + Surface<Error = E1, Connectors = C> + 'static,
    S2: RawSurface + Surface<Error = E2, Connectors = C> + 'static,
    D1: RawDevice<Surface = S1> + 'static,
    D2: RawDevice<Surface = S2> + 'static,
{
    type Surface = FallbackSurface<S1, S2>;
}

#[cfg(feature = "use_system_lib")]
impl<D1: Device + EGLGraphicsBackend + 'static, D2: Device + EGLGraphicsBackend + 'static> EGLGraphicsBackend
    for FallbackDevice<D1, D2>
{
    fallback_device_impl!(bind_wl_display, &Self, Result<EGLBufferReader, EGLError>, display : &Display);
}

impl<E1, E2, C, S1, S2> Surface for FallbackSurface<S1, S2>
where
    // Connectors need to match for both Surfaces
    E1: std::error::Error + Send + 'static,
    E2: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E1, Connectors = C> + 'static,
    S2: Surface<Error = E2, Connectors = C> + 'static,
{
    type Error = EitherError<E1, E2>;
    type Connectors = C;

    fallback_surface_impl!(crtc, &Self, crtc::Handle);
    fallback_surface_impl!(current_connectors, &Self, C);
    fallback_surface_impl!(pending_connectors, &Self, C);
    fallback_surface_err_impl!(add_connector, &Self, Result<(), EitherError<E1, E2>>, conn: connector::Handle);
    fallback_surface_err_impl!(remove_connector, &Self, Result<(), EitherError<E1, E2>>, conn: connector::Handle);
    fallback_surface_err_impl!(set_connectors, &Self, Result<(), EitherError<E1, E2>>, conns: &[connector::Handle]);
    fallback_surface_impl!(current_mode, &Self, Mode);
    fallback_surface_impl!(pending_mode, &Self, Mode);
    fallback_surface_err_impl!(use_mode, &Self, Result<(), EitherError<E1, E2>>, mode: Mode);
}

impl<E1, E2, C, S1, S2> RawSurface for FallbackSurface<S1, S2>
where
    E1: std::error::Error + Send + 'static,
    E2: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: RawSurface + Surface<Error = E1, Connectors = C> + 'static,
    S2: RawSurface + Surface<Error = E2, Connectors = C> + 'static,
{
    fallback_surface_impl!(commit_pending, &Self, bool);
    fallback_surface_err_impl!(commit, &Self, Result<(), EitherError<E1, E2>>, fb: framebuffer::Handle);
    fn page_flip(&self, framebuffer: framebuffer::Handle) -> Result<(), EitherError<E1, E2>> {
        match self {
            FallbackSurface::Preference(dev) => {
                RawSurface::page_flip(dev, framebuffer).map_err(EitherError::Either)
            }
            FallbackSurface::Fallback(dev) => {
                RawSurface::page_flip(dev, framebuffer).map_err(EitherError::Or)
            }
        }
    }
}

impl<S1: Surface + AsRawFd, S2: Surface + AsRawFd> AsRawFd for FallbackSurface<S1, S2> {
    fallback_surface_impl!(as_raw_fd, &Self, RawFd);
}
impl<S1: Surface + BasicDevice, S2: Surface + BasicDevice> BasicDevice for FallbackSurface<S1, S2> {}
impl<S1: Surface + ControlDevice, S2: Surface + ControlDevice> ControlDevice for FallbackSurface<S1, S2> {}

impl<E1, E2, E3, E4, C, CF, S1, S2> CursorBackend for FallbackSurface<S1, S2>
where
    E1: std::error::Error + Send + 'static,
    E2: std::error::Error + Send + 'static,
    E3: std::error::Error + 'static,
    E4: std::error::Error + 'static,
    CF: ?Sized,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E1, Connectors = C> + CursorBackend<CursorFormat = CF, Error = E3> + 'static,
    S2: Surface<Error = E2, Connectors = C> + CursorBackend<CursorFormat = CF, Error = E4> + 'static,
{
    type CursorFormat = CF;
    type Error = EitherError<E3, E4>;

    fallback_surface_err_impl!(set_cursor_position, &Self, Result<(), EitherError<E3, E4>>, x: u32, y: u32);
    fallback_surface_err_impl!(set_cursor_representation, &Self, Result<(), EitherError<E3, E4>>, buffer: &Self::CursorFormat, hotspot: (u32, u32));
}

#[cfg(feature = "renderer_gl")]
impl<E1, E2, C, S1, S2> GLGraphicsBackend for FallbackSurface<S1, S2>
where
    E1: std::error::Error + Send + 'static,
    E2: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E1, Connectors = C> + GLGraphicsBackend + 'static,
    S2: Surface<Error = E2, Connectors = C> + GLGraphicsBackend + 'static,
{
    fallback_surface_impl!(swap_buffers, &Self, Result<(), SwapBuffersError>);
    fallback_surface_impl!(get_proc_address, &Self, *const c_void, symbol: &str);
    fallback_surface_impl!(get_framebuffer_dimensions, &Self, (u32, u32));
    fallback_surface_impl!(is_current, &Self, bool);
    unsafe fn make_current(&self) -> Result<(), SwapBuffersError> {
        match self {
            FallbackSurface::Preference(dev) => dev.make_current(),
            FallbackSurface::Fallback(dev) => dev.make_current(),
        }
    }
    fallback_surface_impl!(get_pixel_format, &Self, PixelFormat);
}
