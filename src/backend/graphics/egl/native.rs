//! Type safe native types for safe context/surface creation

use super::error::*;
use super::ffi;
#[cfg(feature = "backend_drm")]
use backend::drm::error::{Error as DrmError, ErrorKind as DrmErrorKind, Result as DrmResult};
#[cfg(feature = "backend_drm")]
use gbm::{AsRaw, BufferObjectFlags, Device as GbmDevice, Format as GbmFormat, Surface as GbmSurface};
#[cfg(feature = "backend_drm")]
use std::marker::PhantomData;
#[cfg(feature = "backend_drm")]
use std::os::unix::io::AsRawFd;
#[cfg(any(feature = "backend_drm", feature = "backend_winit"))]
use std::ptr;
#[cfg(feature = "backend_winit")]
use wayland_client::egl as wegl;
#[cfg(feature = "backend_winit")]
use winit::os::unix::WindowExt;
#[cfg(feature = "backend_winit")]
use winit::Window as WinitWindow;

/// Trait for typed backend variants (X11/Wayland/GBM)
pub trait Backend {
    /// Surface type created by this backend
    type Surface: NativeSurface;

    /// Return an `EGLDisplay` based on this backend
    ///
    /// # Unsafety
    ///
    /// The returned `EGLDisplay` needs to be a valid ptr for egl,
    /// but there is no way to test that.
    unsafe fn get_display<F: Fn(&str) -> bool>(
        display: ffi::NativeDisplayType,
        has_dp_extension: F,
        log: ::slog::Logger,
    ) -> ffi::egl::types::EGLDisplay;
}

#[cfg(feature = "backend_winit")]
/// Wayland backend type
pub enum Wayland {}
#[cfg(feature = "backend_winit")]
impl Backend for Wayland {
    type Surface = wegl::WlEglSurface;

    unsafe fn get_display<F>(
        display: ffi::NativeDisplayType,
        has_dp_extension: F,
        log: ::slog::Logger,
    ) -> ffi::egl::types::EGLDisplay
    where
        F: Fn(&str) -> bool,
    {
        if has_dp_extension("EGL_KHR_platform_wayland") && ffi::egl::GetPlatformDisplay::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_KHR_platform_wayland");
            ffi::egl::GetPlatformDisplay(ffi::egl::PLATFORM_WAYLAND_KHR, display as *mut _, ptr::null())
        } else if has_dp_extension("EGL_EXT_platform_wayland") && ffi::egl::GetPlatformDisplayEXT::is_loaded()
        {
            trace!(log, "EGL Display Initialization via EGL_EXT_platform_wayland");
            ffi::egl::GetPlatformDisplayEXT(ffi::egl::PLATFORM_WAYLAND_EXT, display as *mut _, ptr::null())
        } else {
            trace!(log, "Default EGL Display Initialization via GetDisplay");
            ffi::egl::GetDisplay(display as *mut _)
        }
    }
}

#[cfg(feature = "backend_winit")]
/// Typed Xlib window for the `X11` backend
pub struct XlibWindow(u64);
#[cfg(feature = "backend_winit")]
/// X11 backend type
pub enum X11 {}
#[cfg(feature = "backend_winit")]
impl Backend for X11 {
    type Surface = XlibWindow;

    unsafe fn get_display<F>(
        display: ffi::NativeDisplayType,
        has_dp_extension: F,
        log: ::slog::Logger,
    ) -> ffi::egl::types::EGLDisplay
    where
        F: Fn(&str) -> bool,
    {
        if has_dp_extension("EGL_KHR_platform_x11") && ffi::egl::GetPlatformDisplay::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_KHR_platform_x11");
            ffi::egl::GetPlatformDisplay(ffi::egl::PLATFORM_X11_KHR, display as *mut _, ptr::null())
        } else if has_dp_extension("EGL_EXT_platform_x11") && ffi::egl::GetPlatformDisplayEXT::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_EXT_platform_x11");
            ffi::egl::GetPlatformDisplayEXT(ffi::egl::PLATFORM_X11_EXT, display as *mut _, ptr::null())
        } else {
            trace!(log, "Default EGL Display Initialization via GetDisplay");
            ffi::egl::GetDisplay(display as *mut _)
        }
    }
}
#[cfg(feature = "backend_drm")]
/// Gbm backend type
pub struct Gbm<T: 'static> {
    _userdata: PhantomData<T>,
}
#[cfg(feature = "backend_drm")]
impl<T: 'static> Backend for Gbm<T> {
    type Surface = GbmSurface<T>;

    unsafe fn get_display<F>(
        display: ffi::NativeDisplayType,
        has_dp_extension: F,
        log: ::slog::Logger,
    ) -> ffi::egl::types::EGLDisplay
    where
        F: Fn(&str) -> bool,
    {
        if has_dp_extension("EGL_KHR_platform_gbm") && ffi::egl::GetPlatformDisplay::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_KHR_platform_gbm");
            ffi::egl::GetPlatformDisplay(ffi::egl::PLATFORM_GBM_KHR, display as *mut _, ptr::null())
        } else if has_dp_extension("EGL_MESA_platform_gbm") && ffi::egl::GetPlatformDisplayEXT::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_MESA_platform_gbm");
            ffi::egl::GetPlatformDisplayEXT(ffi::egl::PLATFORM_GBM_MESA, display as *mut _, ptr::null())
        } else if has_dp_extension("EGL_MESA_platform_gbm") && ffi::egl::GetPlatformDisplay::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_MESA_platform_gbm");
            ffi::egl::GetPlatformDisplay(ffi::egl::PLATFORM_GBM_MESA, display as *mut _, ptr::null())
        } else {
            trace!(log, "Default EGL Display Initialization via GetDisplay");
            ffi::egl::GetDisplay(display as *mut _)
        }
    }
}

/// Trait for types returning Surfaces which can be used to initialize `EGLSurface`s
///
/// ## Unsafety
///
/// The returned `NativeDisplayType` must be valid for egl and there is no way to test that.
pub unsafe trait NativeDisplay<B: Backend> {
    /// Arguments used to surface creation.
    type Arguments;
    /// Error type thrown by the surface creation in case of failure.
    type Error: ::std::error::Error + Send + 'static;
    /// Because one typ might implement multiple `Backend` this function must be called to check
    /// if the expected `Backend` is used at runtime.
    fn is_backend(&self) -> bool;
    /// Return a raw pointer egl will accept for context creation.
    fn ptr(&self) -> Result<ffi::NativeDisplayType>;
    /// Create a surface
    fn create_surface(&self, args: Self::Arguments) -> ::std::result::Result<B::Surface, Self::Error>;
}

#[cfg(feature = "backend_winit")]
unsafe impl NativeDisplay<X11> for WinitWindow {
    type Arguments = ();
    type Error = Error;

    fn is_backend(&self) -> bool {
        self.get_xlib_display().is_some()
    }

    fn ptr(&self) -> Result<ffi::NativeDisplayType> {
        self.get_xlib_display()
            .map(|ptr| ptr as *const _)
            .ok_or(ErrorKind::NonMatchingBackend("X11").into())
    }

    fn create_surface(&self, _args: ()) -> Result<XlibWindow> {
        self.get_xlib_window()
            .map(XlibWindow)
            .ok_or(ErrorKind::NonMatchingBackend("X11").into())
    }
}

#[cfg(feature = "backend_winit")]
unsafe impl NativeDisplay<Wayland> for WinitWindow {
    type Arguments = ();
    type Error = Error;

    fn is_backend(&self) -> bool {
        self.get_wayland_display().is_some()
    }

    fn ptr(&self) -> Result<ffi::NativeDisplayType> {
        self.get_wayland_display()
            .map(|ptr| ptr as *const _)
            .ok_or(ErrorKind::NonMatchingBackend("Wayland").into())
    }

    fn create_surface(&self, _args: ()) -> Result<wegl::WlEglSurface> {
        if let Some(surface) = self.get_wayland_surface() {
            let size = self.get_inner_size().unwrap();
            Ok(unsafe {
                wegl::WlEglSurface::new_from_raw(surface as *mut _, size.width as i32, size.height as i32)
            })
        } else {
            bail!(ErrorKind::NonMatchingBackend("Wayland"))
        }
    }
}

#[cfg(feature = "backend_drm")]
/// Arguments necessary to construct a `GbmSurface`
pub struct GbmSurfaceArguments {
    /// Size of the surface
    pub size: (u32, u32),
    /// Pixel format of the surface
    pub format: GbmFormat,
    /// Flags for surface creation
    pub flags: BufferObjectFlags,
}

#[cfg(feature = "backend_drm")]
unsafe impl<A: AsRawFd + 'static, T: 'static> NativeDisplay<Gbm<T>> for GbmDevice<A> {
    type Arguments = GbmSurfaceArguments;
    type Error = DrmError;

    fn is_backend(&self) -> bool {
        true
    }

    fn ptr(&self) -> Result<ffi::NativeDisplayType> {
        Ok(self.as_raw() as *const _)
    }

    fn create_surface(&self, args: GbmSurfaceArguments) -> DrmResult<GbmSurface<T>> {
        use backend::drm::error::ResultExt as DrmResultExt;

        DrmResultExt::chain_err(
            GbmDevice::create_surface(self, args.size.0, args.size.1, args.format, args.flags),
            || DrmErrorKind::GbmInitFailed,
        )
    }
}

/// Trait for types returning valid surface pointers for initializing egl
///
/// ## Unsafety
///
/// The returned `NativeWindowType` must be valid for egl and there is no way to test that.
pub unsafe trait NativeSurface {
    /// Return a raw pointer egl will accept for surface creation.
    fn ptr(&self) -> ffi::NativeWindowType;
}

#[cfg(feature = "backend_winit")]
unsafe impl NativeSurface for XlibWindow {
    fn ptr(&self) -> ffi::NativeWindowType {
        self.0 as *const _
    }
}

#[cfg(feature = "backend_winit")]
unsafe impl NativeSurface for wegl::WlEglSurface {
    fn ptr(&self) -> ffi::NativeWindowType {
        self.ptr() as *const _
    }
}

#[cfg(feature = "backend_drm")]
unsafe impl<T: 'static> NativeSurface for GbmSurface<T> {
    fn ptr(&self) -> ffi::NativeWindowType {
        self.as_raw() as *const _
    }
}
