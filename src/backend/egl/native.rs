//! Type safe native types for safe context/surface creation

use super::{
    display::EGLDisplayHandle, ffi, wrap_egl_call, SwapBuffersError,
};
use nix::libc::{c_int, c_void};
use std::sync::Arc;
#[cfg(feature = "backend_gbm")]
use std::os::unix::io::AsRawFd;

#[cfg(feature = "backend_winit")]
use wayland_egl as wegl;
#[cfg(feature = "backend_winit")]
use winit::platform::unix::WindowExtUnix;
#[cfg(feature = "backend_winit")]
use winit::window::Window as WinitWindow;

#[cfg(feature = "backend_gbm")]
use gbm::{AsRaw, Device as GbmDevice};

pub trait EGLNativeDisplay: Send {
    fn required_extensions(&self) -> &'static [&'static str];
    fn platform_display(&self) -> (ffi::egl::types::EGLenum, *mut c_void, Vec<ffi::EGLint>);
    /// Type of surfaces created
    fn surface_type(&self) -> ffi::EGLint {
        ffi::egl::WINDOW_BIT as ffi::EGLint
    }
}

#[cfg(feature = "backend_gbm")]
impl<A: AsRawFd + Send + 'static> EGLNativeDisplay for GbmDevice<A> {
    fn required_extensions(&self) -> &'static [&'static str] {
        &["EGL_MESA_platform_gbm"]
    }
    fn platform_display(&self) -> (ffi::egl::types::EGLenum, *mut c_void, Vec<ffi::EGLint>) {
        (ffi::egl::PLATFORM_GBM_MESA, self.as_raw() as *mut _, vec![ffi::egl::NONE as ffi::EGLint])
    }
}

#[cfg(feature = "backend_winit")]
impl EGLNativeDisplay for WinitWindow {
    fn required_extensions(&self) -> &'static [&'static str] {
        if self.wayland_display().is_some() {
            &["EGL_EXT_platform_wayland"]
        } else if self.xlib_display().is_some() {
            &["EGL_EXT_platform_x11"]
        } else {
            unreachable!("No backends for winit other then Wayland and X11 are supported")
        }
    }

    fn platform_display(&self) -> (ffi::egl::types::EGLenum, *mut c_void, Vec<ffi::EGLint>) {
        if let Some(display) = self.wayland_display() {
            (ffi::egl::PLATFORM_WAYLAND_EXT, display as *mut _, vec![ffi::egl::NONE as ffi::EGLint])
        } else if let Some(display) = self.xlib_display() {
            (ffi::egl::PLATFORM_X11_EXT, display as *mut _, vec![ffi::egl::NONE as ffi::EGLint])
        } else {
            unreachable!("No backends for winit other then Wayland and X11 are supported")
        }
    }
}

/// Trait for types returning valid surface pointers for initializing egl
///
/// ## Unsafety
///
/// The returned [`NativeWindowType`](ffi::NativeWindowType) must be valid for EGL
/// and there is no way to test that.
pub unsafe trait EGLNativeSurface: Send + Sync {
    /// Error type thrown by the surface creation in case of failure.
    /// Create an EGLSurface from the internal native type.
    ///
    /// Must be able to deal with re-creation of existing resources,
    /// if `needs_recreation` can return `true`.
    ///
    fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
        surface_attributes: &[c_int],
    ) -> Result<*const c_void, super::EGLError>;

    /// Will be called to check if any internal resources will need
    /// to be recreated. Old resources must be used until `create`
    /// was called again and a new surface was optained.
    ///
    /// Only needs to be recreated, if this may return true.
    /// The default implementation always returns false.
    fn needs_recreation(&self) -> bool {
        false
    }

    /// If the surface supports resizing you may implement and use this function.
    /// 
    /// The two first arguments (width, height) are the new size of the surface,
    /// the two others (dx, dy) represent the displacement of the top-left corner of the surface.
    /// It allows you to control the direction of the resizing if necessary.
    /// 
    /// Implementations may ignore the dx and dy arguments.
    /// 
    /// Returns true if the resize was successful.
    fn resize(&self, _width: i32, _height: i32, _dx: i32, _dy: i32) -> bool {
        false
    }

    /// Adds additional semantics when calling
    /// [EGLSurface::swap_buffers](::backend::egl::surface::EGLSurface::swap_buffers)
    ///
    /// Only implement if required by the backend.
    fn swap_buffers(
        &self,
        display: &Arc<EGLDisplayHandle>,
        surface: ffi::egl::types::EGLSurface,
    ) -> Result<(), SwapBuffersError> {
        wrap_egl_call(|| unsafe {
            ffi::egl::SwapBuffers(***display, surface as *const _);
        })
        .map_err(SwapBuffersError::EGLSwapBuffers)
    }
}

#[cfg(feature = "backend_winit")]
/// Typed Xlib window for the `X11` backend
pub struct XlibWindow(pub u64);

#[cfg(feature = "backend_winit")]
unsafe impl EGLNativeSurface for XlibWindow {
    fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
        surface_attributes: &[c_int],
    ) -> Result<*const c_void, super::EGLError> {
        wrap_egl_call(|| unsafe {
            let mut id = self.0;
            ffi::egl::CreatePlatformWindowSurfaceEXT(
                display.handle,
                config_id,
                (&mut id) as *mut u64 as *mut _,
                surface_attributes.as_ptr(),
            )
        })
    }
}

#[cfg(feature = "backend_winit")]
unsafe impl EGLNativeSurface for wegl::WlEglSurface {
    fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
        surface_attributes: &[c_int],
    ) -> Result<*const c_void, super::EGLError> {
        wrap_egl_call(|| unsafe {
            ffi::egl::CreatePlatformWindowSurfaceEXT(
                display.handle,
                config_id,
                self.ptr() as *mut _,
                surface_attributes.as_ptr(),
            )
        })
    }
    
    fn resize(&self, width: i32, height: i32, dx: i32, dy: i32) -> bool {
        wegl::WlEglSurface::resize(self, width, height, dx, dy);
        true
    }
}
