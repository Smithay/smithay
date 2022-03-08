//! Type safe native types for safe context/surface creation

use super::{display::EGLDisplayHandle, ffi, wrap_egl_call, EGLDevice, SwapBuffersError};
use crate::utils::{Physical, Rectangle};
#[cfg(feature = "backend_winit")]
use std::os::raw::c_int;
use std::os::raw::c_void;
#[cfg(feature = "backend_gbm")]
use std::os::unix::io::AsRawFd;
use std::{fmt::Debug, marker::PhantomData, sync::Arc};

#[cfg(feature = "backend_winit")]
use wayland_egl as wegl;
#[cfg(feature = "backend_winit")]
use winit::{platform::unix::WindowExtUnix, window::Window as WinitWindow};

#[cfg(feature = "backend_gbm")]
use gbm::{AsRaw, Device as GbmDevice};

/// Create a `EGLPlatform<'a>` for the provided platform.
///
/// # Arguments
///
/// * `platform` - The platform defined in `ffi::egl::`
/// * `native_display` - The native display raw pointer which can be casted to `*mut c_void`
/// * `required_extensions` - The name of the required EGL Extension for this platform
///
/// # Optional Arguments
/// * `attrib_list` - A list of `ffi::EGLint` like defined in the EGL Extension
///
/// # Examples
///
/// ```ignore
/// use smithay::backend::egl::{ffi, native::EGLPlatform};
/// use smithay::egl_platform;
///
/// // see: https://www.khronos.org/registry/EGL/extensions/KHR/EGL_KHR_platform_gbm.txt
/// egl_platform!(PLATFORM_GBM_KHR, native_display, &["EGL_KHR_platform_gbm"]);
/// ```
#[macro_export]
macro_rules! egl_platform {
    ($platform:ident, $native_display:expr, $required_extensions:expr) => {
        egl_platform!(
            $platform,
            $native_display,
            $required_extensions,
            vec![ffi::egl::NONE as ffi::EGLint]
        )
    };
    ($platform:ident, $native_display:expr, $required_extensions:expr, $attrib_list:expr) => {
        EGLPlatform::new(
            ffi::egl::$platform,
            stringify!($platform),
            $native_display as *mut _,
            $attrib_list,
            $required_extensions,
        )
    };
}

/// Type, Raw handle and attributes used to call [`eglGetPlatformDisplayEXT`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_base.txt)  
pub struct EGLPlatform<'a> {
    /// Required extensions to use this platform
    pub required_extensions: &'static [&'static str],
    /// Human readable name of the platform
    pub platform_name: &'static str,
    /// Platform type used to call [`eglGetPlatformDisplayEXT`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_base.txt)
    pub platform: ffi::egl::types::EGLenum,
    /// Raw native display handle used to call [`eglGetPlatformDisplayEXT`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_base.txt)
    pub native_display: *mut c_void,
    /// Attributes used to call [`eglGetPlatformDisplayEXT`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_base.txt)
    pub attrib_list: Vec<ffi::EGLint>,
    _phantom: PhantomData<&'a c_void>,
}

impl<'a> EGLPlatform<'a> {
    /// Create a `EGLPlatform<'a>` for the provided platform.
    ///
    /// # Arguments
    ///
    /// * `platform` - The platform defined in `ffi::egl::`
    /// * `platform_name` - A human readable representation of the platform
    /// * `native_display` - The native display raw pointer which can be cast to `*mut c_void`
    /// * `attrib_list` - A list of `ffi::EGLint` like defined in the EGL Extension
    /// * `required_extensions` - The names of the required EGL Extensions for this platform
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use smithay::backend::egl::{ffi, native::EGLPlatform};
    ///
    /// EGLPlatform::new(ffi::egl::PLATFORM_GBM_KHR, "PLATFORM_GBM_KHR", native_display as *mut _, vec![ffi::egl::NONE as ffi::EGLint], &["EGL_KHR_platform_gbm"]);
    /// ```
    pub fn new(
        platform: ffi::egl::types::EGLenum,
        platform_name: &'static str,
        native_display: *mut c_void,
        attrib_list: Vec<ffi::EGLint>,
        required_extensions: &'static [&'static str],
    ) -> Self {
        EGLPlatform {
            platform,
            platform_name,
            native_display,
            attrib_list,
            required_extensions,
            _phantom: PhantomData,
        }
    }
}

impl<'a> Debug for EGLPlatform<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EGLPlatform")
            .field("platform_name", &self.platform_name)
            .field("required_extensions", &self.required_extensions)
            .finish()
    }
}

/// Trait describing platform specific functionality to create a valid `EGLDisplay` using the
/// [`EGL_EXT_platform_base`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_base.txt) extension.
pub trait EGLNativeDisplay: Send {
    /// List of supported platforms that can be used to create a display using
    /// [`eglGetPlatformDisplayEXT`](https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_base.txt)
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>>;

    /// Type of surfaces created
    fn surface_type(&self) -> ffi::EGLint {
        ffi::egl::WINDOW_BIT as ffi::EGLint
    }
}

#[cfg(feature = "backend_gbm")]
impl<A: AsRawFd + Send + 'static> EGLNativeDisplay for GbmDevice<A> {
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>> {
        vec![
            // see: https://www.khronos.org/registry/EGL/extensions/KHR/EGL_KHR_platform_gbm.txt
            egl_platform!(PLATFORM_GBM_KHR, self.as_raw(), &["EGL_KHR_platform_gbm"]),
            // see: https://www.khronos.org/registry/EGL/extensions/MESA/EGL_MESA_platform_gbm.txt
            egl_platform!(PLATFORM_GBM_MESA, self.as_raw(), &["EGL_MESA_platform_gbm"]),
        ]
    }
}

#[cfg(feature = "backend_winit")]
impl EGLNativeDisplay for WinitWindow {
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>> {
        if let Some(display) = self.wayland_display() {
            vec![
                // see: https://www.khronos.org/registry/EGL/extensions/KHR/EGL_KHR_platform_wayland.txt
                egl_platform!(PLATFORM_WAYLAND_KHR, display, &["EGL_KHR_platform_wayland"]),
                // see: https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_wayland.txt
                egl_platform!(PLATFORM_WAYLAND_EXT, display, &["EGL_EXT_platform_wayland"]),
            ]
        } else if let Some(display) = self.xlib_display() {
            vec![
                // see: https://www.khronos.org/registry/EGL/extensions/KHR/EGL_KHR_platform_x11.txt
                egl_platform!(PLATFORM_X11_KHR, display, &["EGL_KHR_platform_x11"]),
                // see: https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_x11.txt
                egl_platform!(PLATFORM_X11_EXT, display, &["EGL_EXT_platform_x11"]),
            ]
        } else {
            unreachable!("No backends for winit other then Wayland and X11 are supported")
        }
    }
}

/// Shallow type for EGL_PLATFORM_X11_EXT with the default X11 display
#[derive(Debug)]
pub struct X11DefaultDisplay;

impl EGLNativeDisplay for X11DefaultDisplay {
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>> {
        vec![egl_platform!(
            PLATFORM_X11_EXT,
            // We pass DEFAULT_DISPLAY (null pointer) because the driver should open a connection to the X server.
            ffi::egl::DEFAULT_DISPLAY,
            &["EGL_EXT_platform_x11"]
        )]
    }
}

impl EGLNativeDisplay for EGLDevice {
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>> {
        // see: https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_device.txt
        vec![egl_platform!(
            PLATFORM_DEVICE_EXT,
            self.inner,
            &["EGL_EXT_platform_device"]
        )]
    }

    fn surface_type(&self) -> ffi::EGLint {
        // EGLDisplays based on EGLDevices do not support normal windowed surfaces.
        // But they may support streams, so lets allow users to create them themselves.
        ffi::egl::STREAM_BIT_KHR as ffi::EGLint
    }
}

/// Trait for types returning valid surface pointers for initializing egl.
///
/// ## Safety
///
/// The returned [`NativeWindowType`](ffi::NativeWindowType) must be valid for EGL
/// and there is no way to test that.
pub unsafe trait EGLNativeSurface: Send {
    /// Create an EGLSurface from the internal native type.
    ///
    /// Must be able to deal with re-creation of existing resources,
    /// if `needs_recreation` can return `true`.
    ///
    fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
    ) -> Result<*const c_void, super::EGLError>;

    /// Will be called to check if any internal resources will need
    /// to be recreated. Old resources must be used until `create`
    /// was called again and a new surface was obtained.
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
    /// [EGLSurface::swap_buffers](crate::backend::egl::surface::EGLSurface::swap_buffers)
    ///
    /// Only implement if required by the backend.
    fn swap_buffers(
        &self,
        display: &Arc<EGLDisplayHandle>,
        surface: ffi::egl::types::EGLSurface,
        damage: Option<&mut [Rectangle<i32, Physical>]>,
    ) -> Result<(), SwapBuffersError> {
        wrap_egl_call(|| unsafe {
            if let Some(damage) = damage {
                ffi::egl::SwapBuffersWithDamageEXT(
                    ***display,
                    surface as *const _,
                    damage.as_mut_ptr() as *mut _,
                    damage.len() as i32,
                );
            } else {
                ffi::egl::SwapBuffers(***display, surface as *const _);
            }
        })
        .map_err(SwapBuffersError::EGLSwapBuffers)
    }
}

#[cfg(feature = "backend_winit")]
static WINIT_SURFACE_ATTRIBUTES: [c_int; 3] = [
    ffi::egl::RENDER_BUFFER as c_int,
    ffi::egl::BACK_BUFFER as c_int,
    ffi::egl::NONE as c_int,
];

#[cfg(feature = "backend_winit")]
/// Typed Xlib window for the `X11` backend
#[derive(Debug)]
pub struct XlibWindow(pub std::os::raw::c_ulong);

#[cfg(feature = "backend_winit")]
unsafe impl EGLNativeSurface for XlibWindow {
    fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
    ) -> Result<*const c_void, super::EGLError> {
        wrap_egl_call(|| unsafe {
            let mut id = self.0;
            ffi::egl::CreatePlatformWindowSurfaceEXT(
                display.handle,
                config_id,
                (&mut id) as *mut std::os::raw::c_ulong as *mut _,
                WINIT_SURFACE_ATTRIBUTES.as_ptr(),
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
    ) -> Result<*const c_void, super::EGLError> {
        wrap_egl_call(|| unsafe {
            ffi::egl::CreatePlatformWindowSurfaceEXT(
                display.handle,
                config_id,
                self.ptr() as *mut _,
                WINIT_SURFACE_ATTRIBUTES.as_ptr(),
            )
        })
    }

    fn resize(&self, width: i32, height: i32, dx: i32, dy: i32) -> bool {
        wegl::WlEglSurface::resize(self, width, height, dx, dy);
        true
    }
}
