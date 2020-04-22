//! Type safe native types for safe context/surface creation

use super::{display::EGLDisplayHandle, ffi, wrap_egl_call, EGLError, Error, SurfaceCreationError};
use nix::libc::{c_int, c_void};

#[cfg(feature = "backend_winit")]
use wayland_egl as wegl;
#[cfg(feature = "backend_winit")]
use winit::platform::unix::WindowExtUnix;
#[cfg(feature = "backend_winit")]
use winit::window::Window as WinitWindow;

/// Trait for typed backend variants (X11/Wayland/GBM)
pub trait Backend {
    /// Surface type created by this backend
    type Surface: NativeSurface<Error = Self::Error>;
    /// Error type thrown by the surface creation in case of failure.
    type Error: ::std::error::Error + Send + 'static;

    /// Return an [`EGLDisplay`](ffi::egl::types::EGLDisplay) based on this backend
    ///
    /// # Safety
    ///
    /// The returned [`EGLDisplay`](ffi::egl::types::EGLDisplay) needs to be a valid pointer for EGL,
    /// but there is no way to test that.
    unsafe fn get_display<F: Fn(&str) -> bool>(
        display: ffi::NativeDisplayType,
        attribs: &[ffi::EGLint],
        has_dp_extension: F,
        log: ::slog::Logger,
    ) -> Result<ffi::egl::types::EGLDisplay, EGLError>;
}

#[cfg(feature = "backend_winit")]
/// Wayland backend type
pub enum Wayland {}
#[cfg(feature = "backend_winit")]
impl Backend for Wayland {
    type Surface = wegl::WlEglSurface;
    type Error = Error;

    unsafe fn get_display<F>(
        display: ffi::NativeDisplayType,
        attribs: &[ffi::EGLint],
        has_dp_extension: F,
        log: ::slog::Logger,
    ) -> Result<ffi::egl::types::EGLDisplay, EGLError>
    where
        F: Fn(&str) -> bool,
    {
        if has_dp_extension("EGL_KHR_platform_wayland") && ffi::egl::GetPlatformDisplay::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_KHR_platform_wayland");
            let attribs = attribs.iter().map(|x| *x as isize).collect::<Vec<_>>();
            wrap_egl_call(|| {
                ffi::egl::GetPlatformDisplay(
                    ffi::egl::PLATFORM_WAYLAND_KHR,
                    display as *mut _,
                    attribs.as_ptr(),
                )
            })
        } else if has_dp_extension("EGL_EXT_platform_wayland") && ffi::egl::GetPlatformDisplayEXT::is_loaded()
        {
            trace!(log, "EGL Display Initialization via EGL_EXT_platform_wayland");
            wrap_egl_call(|| {
                ffi::egl::GetPlatformDisplayEXT(
                    ffi::egl::PLATFORM_WAYLAND_EXT,
                    display as *mut _,
                    attribs.as_ptr(),
                )
            })
        } else {
            trace!(log, "Default EGL Display Initialization via GetDisplay");
            wrap_egl_call(|| ffi::egl::GetDisplay(display as *mut _))
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
    type Error = Error;

    unsafe fn get_display<F>(
        display: ffi::NativeDisplayType,
        attribs: &[ffi::EGLint],
        has_dp_extension: F,
        log: ::slog::Logger,
    ) -> Result<ffi::egl::types::EGLDisplay, EGLError>
    where
        F: Fn(&str) -> bool,
    {
        if has_dp_extension("EGL_KHR_platform_x11") && ffi::egl::GetPlatformDisplay::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_KHR_platform_x11");
            let attribs = attribs.iter().map(|x| *x as isize).collect::<Vec<_>>();
            wrap_egl_call(|| {
                ffi::egl::GetPlatformDisplay(ffi::egl::PLATFORM_X11_KHR, display as *mut _, attribs.as_ptr())
            })
        } else if has_dp_extension("EGL_EXT_platform_x11") && ffi::egl::GetPlatformDisplayEXT::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_EXT_platform_x11");
            wrap_egl_call(|| {
                ffi::egl::GetPlatformDisplayEXT(
                    ffi::egl::PLATFORM_X11_EXT,
                    display as *mut _,
                    attribs.as_ptr(),
                )
            })
        } else {
            trace!(log, "Default EGL Display Initialization via GetDisplay");
            wrap_egl_call(|| ffi::egl::GetDisplay(display as *mut _))
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
    /// Because one type might implement multiple [`Backend`]s this function must be called to check
    /// if the expected [`Backend`] is used at runtime.
    fn is_backend(&self) -> bool;
    /// Return a raw pointer EGL will accept for context creation.
    fn ptr(&self) -> Result<ffi::NativeDisplayType, Error>;
    /// Return attributes that might be used by `B::get_display`
    ///
    /// Default implementation returns an empty list
    fn attributes(&self) -> Vec<ffi::EGLint> {
        vec![ffi::egl::NONE as ffi::EGLint]
    }
    /// Type of surfaces created
    fn surface_type(&self) -> ffi::EGLint {
        ffi::egl::WINDOW_BIT as ffi::EGLint
    }
    /// Create a surface
    fn create_surface(&mut self, args: Self::Arguments) -> Result<B::Surface, B::Error>;
}

#[cfg(feature = "backend_winit")]
unsafe impl NativeDisplay<X11> for WinitWindow {
    type Arguments = ();

    fn is_backend(&self) -> bool {
        self.xlib_display().is_some()
    }

    fn ptr(&self) -> Result<ffi::NativeDisplayType, Error> {
        self.xlib_display()
            .map(|ptr| ptr as *const _)
            .ok_or_else(|| Error::NonMatchingBackend("X11"))
    }

    fn create_surface(&mut self, _args: ()) -> Result<XlibWindow, Error> {
        self.xlib_window()
            .map(XlibWindow)
            .ok_or_else(|| Error::NonMatchingBackend("X11"))
    }
}

#[cfg(feature = "backend_winit")]
unsafe impl NativeDisplay<Wayland> for WinitWindow {
    type Arguments = ();

    fn is_backend(&self) -> bool {
        self.wayland_display().is_some()
    }

    fn ptr(&self) -> Result<ffi::NativeDisplayType, Error> {
        self.wayland_display()
            .map(|ptr| ptr as *const _)
            .ok_or_else(|| Error::NonMatchingBackend("Wayland"))
    }

    fn create_surface(&mut self, _args: ()) -> Result<wegl::WlEglSurface, Error> {
        if let Some(surface) = self.wayland_surface() {
            let size = self.inner_size();
            Ok(unsafe {
                wegl::WlEglSurface::new_from_raw(surface as *mut _, size.width as i32, size.height as i32)
            })
        } else {
            Err(Error::NonMatchingBackend("Wayland"))
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
    /// Error type thrown by the surface creation in case of failure.
    type Error: ::std::error::Error + Send + 'static;
    /// Create an EGLSurface from the internal native type.
    ///
    /// Must be able to deal with re-creation of existing resources,
    /// if `needs_recreation` can return `true`.
    ///
    /// # Safety
    /// This is usually an unsafe operation returning a raw pointer.
    unsafe fn create(
        &self,
        display: &EGLDisplayHandle,
        config_id: ffi::egl::types::EGLConfig,
        surface_attributes: &[c_int],
    ) -> Result<*const c_void, SurfaceCreationError<Self::Error>>;

    /// Will be called to check if any internal resources will need
    /// to be recreated. Old resources must be used until `create`
    /// was called again and a new surface was optained.
    ///
    /// Only needs to be recreated, if this may return true.
    /// The default implementation always returns false.
    fn needs_recreation(&self) -> bool {
        false
    }

    /// Adds additional semantics when calling
    /// [EGLSurface::swap_buffers](::backend::egl::surface::EGLSurface::swap_buffers)
    ///
    /// Only implement if required by the backend.
    fn swap_buffers(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(feature = "backend_winit")]
unsafe impl NativeSurface for XlibWindow {
    type Error = Error;

    unsafe fn create(
        &self,
        display: &EGLDisplayHandle,
        config_id: ffi::egl::types::EGLConfig,
        surface_attributes: &[c_int],
    ) -> Result<*const c_void, SurfaceCreationError<Error>> {
        wrap_egl_call(|| {
            ffi::egl::CreateWindowSurface(
                display.handle,
                config_id,
                self.0 as *const _,
                surface_attributes.as_ptr(),
            )
        })
        .map_err(SurfaceCreationError::EGLSurfaceCreationFailed)
    }
}

#[cfg(feature = "backend_winit")]
unsafe impl NativeSurface for wegl::WlEglSurface {
    type Error = Error;

    unsafe fn create(
        &self,
        display: &EGLDisplayHandle,
        config_id: ffi::egl::types::EGLConfig,
        surface_attributes: &[c_int],
    ) -> Result<*const c_void, SurfaceCreationError<Error>> {
        wrap_egl_call(|| {
            ffi::egl::CreateWindowSurface(
                display.handle,
                config_id,
                self.ptr() as *const _,
                surface_attributes.as_ptr(),
            )
        })
        .map_err(SurfaceCreationError::EGLSurfaceCreationFailed)
    }
}
