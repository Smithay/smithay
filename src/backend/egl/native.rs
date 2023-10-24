//! Type safe native types for safe context/surface creation

#[cfg(feature = "backend_winit")]
use super::wrap_egl_call_ptr;
use super::{
    display::{DamageSupport, EGLDisplayHandle},
    ffi, wrap_egl_call_bool, EGLDevice, SwapBuffersError,
};
#[cfg(feature = "backend_gbm")]
use crate::utils::DevPath;
use crate::utils::{Physical, Rectangle};
#[cfg(feature = "backend_winit")]
use std::os::raw::c_int;
use std::os::raw::c_void;
#[cfg(feature = "backend_gbm")]
use std::os::unix::io::AsFd;
use std::{fmt::Debug, marker::PhantomData, sync::Arc};

#[cfg(feature = "backend_winit")]
use wayland_egl as wegl;
#[cfg(feature = "backend_winit")]
use winit::window::Window as WinitWindow;

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

    /// String identifying this native display from its counterparts of the same platform, if applicable.
    fn identifier(&self) -> Option<String> {
        None
    }
}

#[cfg(feature = "backend_gbm")]
impl<A: AsFd + Send + 'static> EGLNativeDisplay for GbmDevice<A> {
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>> {
        vec![
            // see: https://www.khronos.org/registry/EGL/extensions/KHR/EGL_KHR_platform_gbm.txt
            egl_platform!(PLATFORM_GBM_KHR, self.as_raw(), &["EGL_KHR_platform_gbm"]),
            // see: https://www.khronos.org/registry/EGL/extensions/MESA/EGL_MESA_platform_gbm.txt
            egl_platform!(PLATFORM_GBM_MESA, self.as_raw(), &["EGL_MESA_platform_gbm"]),
        ]
    }

    fn identifier(&self) -> Option<String> {
        self.dev_path().map(|p| p.to_string_lossy().into_owned())
    }
}

#[cfg(feature = "backend_winit")]
impl EGLNativeDisplay for Arc<WinitWindow> {
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>> {
        use winit::raw_window_handle::{self, HasDisplayHandle};

        match self.display_handle().map(|handle| handle.as_raw()) {
            Ok(raw_window_handle::RawDisplayHandle::Xlib(handle)) => {
                let display = handle.display.unwrap().as_ptr();
                vec![
                    // see: https://www.khronos.org/registry/EGL/extensions/KHR/EGL_KHR_platform_x11.txt
                    egl_platform!(PLATFORM_X11_KHR, display, &["EGL_KHR_platform_x11"]),
                    // see: https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_x11.txt
                    egl_platform!(PLATFORM_X11_EXT, display, &["EGL_EXT_platform_x11"]),
                    // see: https://raw.githubusercontent.com/google/angle/main/extensions/EGL_ANGLE_platform_angle.txt
                    egl_platform!(
                        PLATFORM_ANGLE_ANGLE,
                        display,
                        &["EGL_ANGLE_platform_angle", "EGL_ANGLE_platform_angle_vulkan"],
                        vec![
                            ffi::egl::PLATFORM_ANGLE_NATIVE_PLATFORM_TYPE_ANGLE,
                            ffi::egl::PLATFORM_X11_EXT as _,
                            ffi::egl::PLATFORM_ANGLE_TYPE_ANGLE,
                            ffi::egl::PLATFORM_ANGLE_TYPE_VULKAN_ANGLE,
                            ffi::egl::NONE as ffi::EGLint
                        ]
                    ),
                ]
            }
            Ok(raw_window_handle::RawDisplayHandle::Wayland(handle)) => {
                let display = handle.display.as_ptr();
                vec![
                    // see: https://www.khronos.org/registry/EGL/extensions/KHR/EGL_KHR_platform_wayland.txt
                    egl_platform!(PLATFORM_WAYLAND_KHR, display, &["EGL_KHR_platform_wayland"]),
                    // see: https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_wayland.txt
                    egl_platform!(PLATFORM_WAYLAND_EXT, display, &["EGL_EXT_platform_wayland"]),
                    // see: https://raw.githubusercontent.com/google/angle/main/extensions/EGL_ANGLE_platform_angle.txt
                    egl_platform!(
                        PLATFORM_ANGLE_ANGLE,
                        display,
                        &[
                            "EGL_ANGLE_platform_angle",
                            "EGL_ANGLE_platform_angle_vulkan",
                            "EGL_EXT_platform_wayland",
                        ],
                        vec![
                            ffi::egl::PLATFORM_ANGLE_NATIVE_PLATFORM_TYPE_ANGLE,
                            ffi::egl::PLATFORM_WAYLAND_EXT as _,
                            ffi::egl::PLATFORM_ANGLE_TYPE_ANGLE,
                            ffi::egl::PLATFORM_ANGLE_TYPE_VULKAN_ANGLE,
                            ffi::egl::NONE as ffi::EGLint
                        ]
                    ),
                ]
            }
            _ => unreachable!("No backends for winit other then Wayland and X11 are supported"),
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

    fn identifier(&self) -> Option<String> {
        self.render_device_path()
            .ok()
            .or_else(|| self.drm_device_path().ok())
            .map(|p| p.to_string_lossy().into_owned())
    }
}

/// Shallow type for the EGL_PLATFORM_SURFACELESS with default EGL display
#[derive(Debug)]
pub struct EGLSurfacelessDisplay;

impl EGLNativeDisplay for EGLSurfacelessDisplay {
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>> {
        vec![
            // see: https://www.khronos.org/registry/EGL/extensions/MESA/EGL_MESA_platform_surfaceless.txt
            egl_platform!(
                PLATFORM_SURFACELESS_MESA,
                ffi::egl::DEFAULT_DISPLAY,
                &["EGL_MESA_platform_surfaceless"]
            ),
        ]
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
    /// # Safety
    ///
    /// - `config_id` has to represent a valid config
    unsafe fn create(
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
    #[profiling::function]
    fn swap_buffers(
        &self,
        display: &Arc<EGLDisplayHandle>,
        surface: ffi::egl::types::EGLSurface,
        damage: Option<&mut [Rectangle<i32, Physical>]>,
        damage_impl: DamageSupport,
    ) -> Result<(), SwapBuffersError> {
        wrap_egl_call_bool(|| unsafe {
            if let Some(damage) = damage {
                match damage_impl {
                    DamageSupport::KHR => ffi::egl::SwapBuffersWithDamageKHR(
                        ***display,
                        surface as *const _,
                        damage.as_mut_ptr() as *mut _,
                        damage.len() as i32,
                    ),
                    DamageSupport::EXT => ffi::egl::SwapBuffersWithDamageEXT(
                        ***display,
                        surface as *const _,
                        damage.as_mut_ptr() as *mut _,
                        damage.len() as i32,
                    ),
                    DamageSupport::No => ffi::egl::SwapBuffers(***display, surface as *const _),
                }
            } else {
                ffi::egl::SwapBuffers(***display, surface as *const _)
            }
        })
        .map_err(SwapBuffersError::EGLSwapBuffers)
        .map(|_| ())
    }

    /// String identifying this native surface from its counterparts of the same platform, if applicable.
    fn identifier(&self) -> Option<String> {
        None
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
    unsafe fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
    ) -> Result<*const c_void, super::EGLError> {
        wrap_egl_call_ptr(|| unsafe {
            let mut id = self.0;
            ffi::egl::CreatePlatformWindowSurfaceEXT(
                display.handle,
                config_id,
                (&mut id) as *mut std::os::raw::c_ulong as *mut _,
                WINIT_SURFACE_ATTRIBUTES.as_ptr(),
            )
        })
    }

    fn identifier(&self) -> Option<String> {
        Some("Winit/X11".into())
    }
}

#[cfg(feature = "backend_winit")]
unsafe impl EGLNativeSurface for wegl::WlEglSurface {
    unsafe fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
    ) -> Result<*const c_void, super::EGLError> {
        wrap_egl_call_ptr(|| unsafe {
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

    fn identifier(&self) -> Option<String> {
        Some("Winit/Wayland".into())
    }
}
