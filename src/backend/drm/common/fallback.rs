//!
//! Types to make fallback device initialization easier
//!

#[cfg(all(feature = "backend_drm_atomic", feature = "backend_drm_legacy"))]
use crate::backend::drm::{atomic::AtomicDrmDevice, legacy::LegacyDrmDevice};
use crate::backend::drm::{common::Error, Device, DeviceHandler, RawDevice, RawSurface, Surface};
use crate::backend::egl::Error as EGLError;
#[cfg(feature = "use_system_lib")]
use crate::backend::egl::{display::EGLBufferReader, EGLGraphicsBackend};
#[cfg(feature = "renderer_gl")]
use crate::backend::graphics::gl::GLGraphicsBackend;
#[cfg(feature = "renderer_gl")]
use crate::backend::graphics::PixelFormat;
use crate::backend::graphics::{CursorBackend, SwapBuffersError};
use crate::backend::session::{AsSessionObserver, SessionObserver};

use drm::{
    control::{connector, crtc, encoder, framebuffer, plane, Device as ControlDevice, Mode, ResourceHandles},
    Device as BasicDevice, SystemError as DrmError,
};
#[cfg(feature = "renderer_gl")]
use nix::libc::c_void;
use nix::libc::dev_t;
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

struct FallbackDeviceHandlerD1<E, C, S1, S2, D1, D2>(
    Box<dyn DeviceHandler<Device = FallbackDevice<D1, D2>> + 'static>,
)
where
    E: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E, Connectors = C> + 'static,
    S2: Surface<Error = E, Connectors = C> + 'static,
    D1: Device<Surface = S1> + 'static,
    D2: Device<Surface = S2> + 'static;

impl<E, C, S1, S2, D1, D2> DeviceHandler for FallbackDeviceHandlerD1<E, C, S1, S2, D1, D2>
where
    E: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E, Connectors = C> + 'static,
    S2: Surface<Error = E, Connectors = C> + 'static,
    D1: Device<Surface = S1> + 'static,
    D2: Device<Surface = S2> + 'static,
{
    type Device = D1;

    fn vblank(&mut self, crtc: crtc::Handle) {
        self.0.vblank(crtc)
    }
    fn error(&mut self, error: E) {
        self.0.error(error);
    }
}

struct FallbackDeviceHandlerD2<E, C, S1, S2, D1, D2>(
    Box<dyn DeviceHandler<Device = FallbackDevice<D1, D2>> + 'static>,
)
where
    E: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E, Connectors = C> + 'static,
    S2: Surface<Error = E, Connectors = C> + 'static,
    D1: Device<Surface = S1> + 'static,
    D2: Device<Surface = S2> + 'static;

impl<E, C, S1, S2, D1, D2> DeviceHandler for FallbackDeviceHandlerD2<E, C, S1, S2, D1, D2>
where
    E: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E, Connectors = C> + 'static,
    S2: Surface<Error = E, Connectors = C> + 'static,
    D1: Device<Surface = S1> + 'static,
    D2: Device<Surface = S2> + 'static,
{
    type Device = D2;

    fn vblank(&mut self, crtc: crtc::Handle) {
        self.0.vblank(crtc)
    }
    fn error(&mut self, error: E) {
        self.0.error(error);
    }
}

/// [`SessionObserver`](::backend::session::SessionObserver) Wrapper to assist fallback
/// in case initialization of the preferred device type fails.
pub enum FallbackDeviceObserver<O1: SessionObserver + 'static, O2: SessionObserver + 'static> {
    /// Variant for successful initialization of the preferred device
    Preference(O1),
    /// Variant for the fallback device
    Fallback(O2),
}

impl<O1, O2, D1, D2> AsSessionObserver<FallbackDeviceObserver<O1, O2>> for FallbackDevice<D1, D2>
where
    O1: SessionObserver + 'static,
    O2: SessionObserver + 'static,
    D1: Device + AsSessionObserver<O1> + 'static,
    D2: Device + AsSessionObserver<O2> + 'static,
{
    fn observer(&mut self) -> FallbackDeviceObserver<O1, O2> {
        match self {
            FallbackDevice::Preference(dev) => FallbackDeviceObserver::Preference(dev.observer()),
            FallbackDevice::Fallback(dev) => FallbackDeviceObserver::Fallback(dev.observer()),
        }
    }
}

impl<O1: SessionObserver + 'static, O2: SessionObserver + 'static> SessionObserver
    for FallbackDeviceObserver<O1, O2>
{
    fn pause(&mut self, device: Option<(u32, u32)>) {
        match self {
            FallbackDeviceObserver::Preference(dev) => dev.pause(device),
            FallbackDeviceObserver::Fallback(dev) => dev.pause(device),
        }
    }

    fn activate(&mut self, device: Option<(u32, u32, Option<RawFd>)>) {
        match self {
            FallbackDeviceObserver::Preference(dev) => dev.activate(device),
            FallbackDeviceObserver::Fallback(dev) => dev.activate(device),
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

#[cfg(all(feature = "backend_drm_atomic", feature = "backend_drm_legacy"))]
impl<A: AsRawFd + Clone + 'static> FallbackDevice<AtomicDrmDevice<A>, LegacyDrmDevice<A>> {
    /// Try to initialize an [`AtomicDrmDevice`](::backend::drm:;atomic::AtomicDrmDevice)
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

        match AtomicDrmDevice::new(fd.clone(), disable_connectors, log.clone()) {
            Ok(dev) => Ok(FallbackDevice::Preference(dev)),
            Err(err) => {
                error!(log, "Failed to initialize preferred AtomicDrmDevice: {}", err);
                info!(log, "Falling back to fallback LegacyyDrmDevice");
                Ok(FallbackDevice::Fallback(LegacyDrmDevice::new(fd, disable_connectors, log)?))
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

impl<D1: Device, D2: Device> AsRawFd for FallbackDevice<D1, D2> {
    fallback_device_impl!(as_raw_fd, &Self, RawFd);
}
impl<D1: Device + BasicDevice, D2: Device + BasicDevice> BasicDevice for FallbackDevice<D1, D2> {}
impl<D1: Device + ControlDevice, D2: Device + ControlDevice> ControlDevice for FallbackDevice<D1, D2> {}

impl<E, C, S1, S2, D1, D2> Device for FallbackDevice<D1, D2>
where
    // Connectors and Error need to match for both Surfaces
    E: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E, Connectors = C> + 'static,
    S2: Surface<Error = E, Connectors = C> + 'static,
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
    fn create_surface(&mut self, crtc: crtc::Handle, mode: Mode, connectors: &[connector::Handle]) -> Result<Self::Surface, E> {
        match self {
            FallbackDevice::Preference(dev) => Ok(FallbackSurface::Preference(dev.create_surface(crtc, mode, connectors)?)),
            FallbackDevice::Fallback(dev) => Ok(FallbackSurface::Fallback(dev.create_surface(crtc, mode, connectors)?)),
        }
    }
    fallback_device_impl!(process_events, &mut Self);
    fallback_device_impl!(resource_handles, &Self, Result<ResourceHandles, E>);
    fallback_device_impl!(get_connector_info, &Self, Result<connector::Info, DrmError>, conn: connector::Handle);
    fallback_device_impl!(get_crtc_info, &Self, Result<crtc::Info, DrmError>, crtc: crtc::Handle);
    fallback_device_impl!(get_encoder_info, &Self, Result<encoder::Info, DrmError>, enc: encoder::Handle);
    fallback_device_impl!(get_framebuffer_info, &Self, Result<framebuffer::Info, DrmError>, fb: framebuffer::Handle);
    fallback_device_impl!(get_plane_info, &Self, Result<plane::Info, DrmError>, plane : plane::Handle);
}

// Impl RawDevice where underlying types implement RawDevice
impl<E, C, S1, S2, D1, D2> RawDevice for FallbackDevice<D1, D2>
where
    // Connectors and Error need to match for both Surfaces
    E: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: RawSurface + Surface<Error = E, Connectors = C> + 'static,
    S2: RawSurface + Surface<Error = E, Connectors = C> + 'static,
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

impl<E, C, S1, S2> Surface for FallbackSurface<S1, S2>
where
    // Connectors and Error need to match for both Surfaces
    E: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E, Connectors = C> + 'static,
    S2: Surface<Error = E, Connectors = C> + 'static,
{
    type Error = E;
    type Connectors = C;

    fallback_surface_impl!(crtc, &Self, crtc::Handle);
    fallback_surface_impl!(current_connectors, &Self, C);
    fallback_surface_impl!(pending_connectors, &Self, C);
    fallback_surface_impl!(add_connector, &Self, Result<(), E>, conn: connector::Handle);
    fallback_surface_impl!(remove_connector, &Self, Result<(), E>, conn: connector::Handle);
    fallback_surface_impl!(set_connectors, &Self, Result<(), E>, conns: &[connector::Handle]);
    fallback_surface_impl!(current_mode, &Self, Mode);
    fallback_surface_impl!(pending_mode, &Self, Mode);
    fallback_surface_impl!(use_mode, &Self, Result<(), E>, mode: Mode);
}

impl<E, C, S1, S2> RawSurface for FallbackSurface<S1, S2>
where
    E: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: RawSurface + Surface<Error = E, Connectors = C> + 'static,
    S2: RawSurface + Surface<Error = E, Connectors = C> + 'static,
{
    fallback_surface_impl!(commit_pending, &Self, bool);
    fallback_surface_impl!(commit, &Self, Result<(), E>, fb: framebuffer::Handle);
    fn page_flip(&self, framebuffer: framebuffer::Handle) -> Result<(), E> {
        match self {
            FallbackSurface::Preference(dev) => RawSurface::page_flip(dev, framebuffer),
            FallbackSurface::Fallback(dev) => RawSurface::page_flip(dev, framebuffer),
        }
    }
}

impl<S1: Surface + AsRawFd, S2: Surface + AsRawFd> AsRawFd for FallbackSurface<S1, S2> {
    fallback_surface_impl!(as_raw_fd, &Self, RawFd);
}
impl<S1: Surface + BasicDevice, S2: Surface + BasicDevice> BasicDevice for FallbackSurface<S1, S2> {}
impl<S1: Surface + ControlDevice, S2: Surface + ControlDevice> ControlDevice for FallbackSurface<S1, S2> {}

impl<E1, E2, C, CF, S1, S2> CursorBackend for FallbackSurface<S1, S2>
where
    E1: std::error::Error + Send + 'static,
    E2: 'static,
    CF: ?Sized,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E1, Connectors = C> + CursorBackend<CursorFormat = CF, Error = E2> + 'static,
    S2: Surface<Error = E1, Connectors = C> + CursorBackend<CursorFormat = CF, Error = E2> + 'static,
{
    type CursorFormat = CF;
    type Error = E2;

    fallback_surface_impl!(set_cursor_position, &Self, Result<(), E2>, x: u32, y: u32);
    fallback_surface_impl!(set_cursor_representation, &Self, Result<(), E2>, buffer: &Self::CursorFormat, hotspot: (u32, u32));
}

#[cfg(feature = "renderer_gl")]
impl<E, C, S1, S2> GLGraphicsBackend for FallbackSurface<S1, S2>
where
    E: std::error::Error + Send + 'static,
    C: IntoIterator<Item = connector::Handle> + 'static,
    S1: Surface<Error = E, Connectors = C> + GLGraphicsBackend + 'static,
    S2: Surface<Error = E, Connectors = C> + GLGraphicsBackend + 'static,
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
