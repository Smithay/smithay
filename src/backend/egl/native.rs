//! Type safe native types for safe context/surface creation

use super::{error::*, ffi};
use crate::backend::graphics::SwapBuffersError;

#[cfg(feature = "backend_winit")]
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

    /// Return an [`EGLDisplay`](ffi::egl::types::EGLDisplay) based on this backend
    ///
    /// # Unsafety
    ///
    /// The returned [`EGLDisplay`](ffi::egl::types::EGLDisplay) needs to be a valid pointer for EGL,
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

/// Trait for types returning Surfaces which can be used to initialize [`EGLSurface`](super::EGLSurface)s
///
/// ## Unsafety
///
/// The returned [`NativeDisplayType`](super::ffi::NativeDisplayType) must be valid for EGL and there is no way to test that.
pub unsafe trait NativeDisplay<B: Backend> {
    /// Arguments used to surface creation.
    type Arguments;
    /// Error type thrown by the surface creation in case of failure.
    type Error: ::std::error::Error + Send + 'static;
    /// Because one type might implement multiple [`Backend`]s this function must be called to check
    /// if the expected [`Backend`] is used at runtime.
    fn is_backend(&self) -> bool;
    /// Return a raw pointer EGL will accept for context creation.
    fn ptr(&self) -> Result<ffi::NativeDisplayType>;
    /// Create a surface
    fn create_surface(&mut self, args: Self::Arguments) -> ::std::result::Result<B::Surface, Self::Error>;
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
            .ok_or_else(|| ErrorKind::NonMatchingBackend("X11").into())
    }

    fn create_surface(&mut self, _args: ()) -> Result<XlibWindow> {
        self.get_xlib_window()
            .map(XlibWindow)
            .ok_or_else(|| ErrorKind::NonMatchingBackend("X11").into())
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
            .ok_or_else(|| ErrorKind::NonMatchingBackend("Wayland").into())
    }

    fn create_surface(&mut self, _args: ()) -> Result<wegl::WlEglSurface> {
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

/// Trait for types returning valid surface pointers for initializing egl
///
/// ## Unsafety
///
/// The returned [`NativeWindowType`](ffi::NativeWindowType) must be valid for EGL
/// and there is no way to test that.
pub unsafe trait NativeSurface {
    /// Return a raw pointer egl will accept for surface creation.
    fn ptr(&self) -> ffi::NativeWindowType;

    /// Will be called to check if any internal resources will need
    /// to be recreated. Old resources must be used until `recreate`
    /// was called.
    ///
    /// Only needs to be recreated, if this shall sometimes return true.
    /// The default implementation always returns false.
    fn needs_recreation(&self) -> bool {
        false
    }

    /// Instructs the surface to recreate internal resources
    ///
    /// Must only be implemented if `needs_recreation` can return `true`.
    /// Returns true on success.
    /// If this call was successful `ptr()` *should* return something different.
    fn recreate(&self) -> bool {
        true
    }

    /// Adds additional semantics when calling
    /// [EGLSurface::swap_buffers](::backend::egl::surface::EGLSurface::swap_buffers)
    ///
    /// Only implement if required by the backend.
    fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError> {
        Ok(())
    }
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
