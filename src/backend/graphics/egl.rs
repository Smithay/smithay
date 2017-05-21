//! Common traits and types for egl context creation and rendering

/// Large parts of the following file are taken from
/// https://github.com/tomaka/glutin/tree/044e651edf67a2029eecc650dd42546af1501414/src/api/egl/
///
/// It therefore falls under glutin's Apache 2.0 license
/// (see https://github.com/tomaka/glutin/tree/044e651edf67a2029eecc650dd42546af1501414/LICENSE)

use super::GraphicsBackend;

use libloading::Library;
use nix::{c_int, c_void};
use std::error::{self, Error};

use std::ffi::{CStr, CString};
use std::fmt;
use std::io;
use std::mem;
use std::ptr;

#[allow(non_camel_case_types, dead_code)]
mod ffi {
    use nix::c_void;
    use nix::libc::{c_long, int32_t, uint64_t};

    pub type khronos_utime_nanoseconds_t = khronos_uint64_t;
    pub type khronos_uint64_t = uint64_t;
    pub type khronos_ssize_t = c_long;
    pub type EGLint = int32_t;
    pub type EGLNativeDisplayType = NativeDisplayType;
    pub type EGLNativePixmapType = NativePixmapType;
    pub type EGLNativeWindowType = NativeWindowType;
    pub type NativeDisplayType = *const c_void;
    pub type NativePixmapType = *const c_void;
    pub type NativeWindowType = *const c_void;

    pub mod egl {
        use super::*;

        include!(concat!(env!("OUT_DIR"), "/egl_bindings.rs"));
    }
}

/// Native types to create an `EGLContext` from.
/// Currently supported providers are X11, Wayland and GBM.
#[derive(Clone, Copy)]
pub enum Native {
    /// X11 Display and Window objects to create an `EGLContext` upon.
    X11(ffi::NativeDisplayType, ffi::NativeWindowType),
    /// Wayland Display and Surface objects to create an `EGLContext` upon.
    Wayland(ffi::NativeDisplayType, ffi::NativeWindowType),
    /// GBM Display
    Gbm(ffi::NativeDisplayType, ffi::NativeWindowType),
}

/// Error that can happen while creating an `EGLContext`
#[derive(Debug)]
pub enum CreationError {
    /// I/O error from the underlying system
    IoError(io::Error),
    /// Operating System error
    OsError(String),
    /// Robustness was requested but is not supported by the graphics system
    RobustnessNotSupported,
    /// The requested OpenGl version is not supported by the graphics system
    OpenGlVersionNotSupported,
    /// There is no pixel format available that fulfills all requirements
    NoAvailablePixelFormat,
    /// Context creation is not supported on this system
    NotSupported,
}

impl From<io::Error> for CreationError {
    fn from(err: io::Error) -> Self {
        CreationError::IoError(err)
    }
}

impl fmt::Display for CreationError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        formatter.write_str(self.description())?;
        if let Some(err) = error::Error::cause(self) {
            write!(formatter, ": {}", err)?;
        }
        Ok(())
    }
}

impl error::Error for CreationError {
    fn description(&self) -> &str {
        match *self {
            CreationError::IoError(ref err) => err.description(),
            CreationError::OsError(ref text) => text,
            CreationError::RobustnessNotSupported => {
                "You requested robustness, but it is \
                                                      not supported."
            }
            CreationError::OpenGlVersionNotSupported => {
                "The requested OpenGL version is not \
                                                         supported."
            }
            CreationError::NoAvailablePixelFormat => {
                "Couldn't find any pixel format that matches \
                                                      the criterias."
            }
            CreationError::NotSupported => "Context creation is not supported on the current window system",
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        match *self {
            CreationError::IoError(ref err) => Some(err),
            _ => None,
        }
    }
}

/// EGL context for rendering
pub struct EGLContext {
    context: *const c_void,
    display: *const c_void,
    egl: ffi::egl::Egl,
    surface: *const c_void,
    pixel_format: PixelFormat,
}

impl EGLContext {
    /// Create a new EGL context
    ///
    /// # Unsafety
    ///
    /// This method is marked unsafe, because the contents of `Native` cannot be verified and msy
    /// contain dangeling pointers are similar unsafe content
    pub unsafe fn new(native: Native, mut attributes: GlAttributes, reqs: PixelFormatRequirements)
                      -> Result<EGLContext, CreationError> {
        let lib = Library::new("libEGL.so.1")?;
        let egl = ffi::egl::Egl::load_with(|sym| {
                                               let name = CString::new(sym).unwrap();
                                               let symbol = lib.get::<*mut c_void>(name.as_bytes());
                                               match symbol {
                                                   Ok(x) => *x as *const _,
                                                   Err(_) => ptr::null(),
                                               }
                                           });

        // If no version is given, try OpenGLES 3.0, if available,
        // fallback to 2.0 otherwise
        let version = match attributes.version {
            Some((3, x)) => (3, x),
            Some((2, x)) => (2, x),
            None => {
                attributes.version = Some((3, 0));
                match EGLContext::new(native, attributes, reqs) {
                    Ok(x) => return Ok(x),
                    Err(_) => {
                        // TODO log
                        attributes.version = Some((2, 0));
                        return EGLContext::new(native, attributes, reqs);
                    }
                }
            }
            Some((1, _)) => {
                // TODO logging + error, 1.0 not supported
                unimplemented!()
            }
            Some(_) => {
                // TODO logging + error, version not supported
                unimplemented!()
            }
        };

        // the first step is to query the list of extensions without any display, if supported
        let dp_extensions = {
            let p = egl.QueryString(ffi::egl::NO_DISPLAY, ffi::egl::EXTENSIONS as i32);

            // this possibility is available only with EGL 1.5 or EGL_EXT_platform_base, otherwise
            // `eglQueryString` returns an error
            if p.is_null() {
                vec![]
            } else {
                let p = CStr::from_ptr(p);
                let list = String::from_utf8(p.to_bytes().to_vec()).unwrap_or_else(|_| String::new());
                list.split(' ').map(|e| e.to_string()).collect::<Vec<_>>()
            }
        };

        let has_dp_extension = |e: &str| dp_extensions.iter().any(|s| s == e);

        let display = match native {
            Native::X11(display, _) if has_dp_extension("EGL_KHR_platform_x11") &&
                                       egl.GetPlatformDisplay.is_loaded() => {
                egl.GetPlatformDisplay(ffi::egl::PLATFORM_X11_KHR, display as *mut _, ptr::null())
            }

            Native::X11(display, _) if has_dp_extension("EGL_EXT_platform_x11") &&
                                       egl.GetPlatformDisplayEXT.is_loaded() => {
                egl.GetPlatformDisplayEXT(ffi::egl::PLATFORM_X11_EXT, display as *mut _, ptr::null())
            }

            Native::Gbm(display, _) if has_dp_extension("EGL_KHR_platform_gbm") &&
                                       egl.GetPlatformDisplay.is_loaded() => {
                egl.GetPlatformDisplay(ffi::egl::PLATFORM_GBM_KHR, display as *mut _, ptr::null())
            }

            Native::Gbm(display, _) if has_dp_extension("EGL_MESA_platform_gbm") &&
                                       egl.GetPlatformDisplayEXT.is_loaded() => {
                egl.GetPlatformDisplayEXT(ffi::egl::PLATFORM_GBM_KHR, display as *mut _, ptr::null())
            }

            Native::Wayland(display, _) if has_dp_extension("EGL_KHR_platform_wayland") &&
                                           egl.GetPlatformDisplay.is_loaded() => {
                egl.GetPlatformDisplay(ffi::egl::PLATFORM_WAYLAND_KHR,
                                       display as *mut _,
                                       ptr::null())
            }

            Native::Wayland(display, _) if has_dp_extension("EGL_EXT_platform_wayland") &&
                                           egl.GetPlatformDisplayEXT.is_loaded() => {
                egl.GetPlatformDisplayEXT(ffi::egl::PLATFORM_WAYLAND_EXT,
                                          display as *mut _,
                                          ptr::null())
            }

            Native::X11(display, _) |
            Native::Gbm(display, _) |
            Native::Wayland(display, _) => egl.GetDisplay(display as *mut _),
        };

        let egl_version = {
            let mut major: ffi::egl::types::EGLint = mem::uninitialized();
            let mut minor: ffi::egl::types::EGLint = mem::uninitialized();

            if egl.Initialize(display, &mut major, &mut minor) == 0 {
                return Err(CreationError::OsError(String::from("eglInitialize failed")));
            }

            (major, minor)
        };

        // the list of extensions supported by the client once initialized is different from the
        // list of extensions obtained earlier
        let extensions = if egl_version >= (1, 2) {
            let p = CStr::from_ptr(egl.QueryString(display, ffi::egl::EXTENSIONS as i32));
            let list = String::from_utf8(p.to_bytes().to_vec()).unwrap_or_else(|_| String::new());
            list.split(' ').map(|e| e.to_string()).collect::<Vec<_>>()

        } else {
            vec![]
        };

        if egl_version >= (1, 2) && egl.BindAPI(ffi::egl::OPENGL_ES_API) == 0 {
            return Err(CreationError::OpenGlVersionNotSupported);
        }

        let descriptor = {
            let mut out: Vec<c_int> = Vec::with_capacity(37);

            if egl_version >= (1, 2) {
                out.push(ffi::egl::COLOR_BUFFER_TYPE as c_int);
                out.push(ffi::egl::RGB_BUFFER as c_int);
            }

            out.push(ffi::egl::SURFACE_TYPE as c_int);
            // TODO: Some versions of Mesa report a BAD_ATTRIBUTE error
            // if we ask for PBUFFER_BIT as well as WINDOW_BIT
            out.push((ffi::egl::WINDOW_BIT) as c_int);

            match version {
                (3, _) => {
                    if egl_version < (1, 3) {
                        return Err(CreationError::NoAvailablePixelFormat);
                    }
                    out.push(ffi::egl::RENDERABLE_TYPE as c_int);
                    out.push(ffi::egl::OPENGL_ES3_BIT as c_int);
                    out.push(ffi::egl::CONFORMANT as c_int);
                    out.push(ffi::egl::OPENGL_ES3_BIT as c_int);
                }
                (2, _) => {
                    if egl_version < (1, 3) {
                        return Err(CreationError::NoAvailablePixelFormat);
                    }
                    out.push(ffi::egl::RENDERABLE_TYPE as c_int);
                    out.push(ffi::egl::OPENGL_ES2_BIT as c_int);
                    out.push(ffi::egl::CONFORMANT as c_int);
                    out.push(ffi::egl::OPENGL_ES2_BIT as c_int);
                }
                (_, _) => unreachable!(),
            };

            if let Some(hardware_accelerated) = reqs.hardware_accelerated {
                out.push(ffi::egl::CONFIG_CAVEAT as c_int);
                out.push(if hardware_accelerated {
                             ffi::egl::NONE as c_int
                         } else {
                             ffi::egl::SLOW_CONFIG as c_int
                         });
            }

            if let Some(color) = reqs.color_bits {
                out.push(ffi::egl::RED_SIZE as c_int);
                out.push((color / 3) as c_int);
                out.push(ffi::egl::GREEN_SIZE as c_int);
                out.push((color / 3 + if color % 3 != 0 { 1 } else { 0 }) as c_int);
                out.push(ffi::egl::BLUE_SIZE as c_int);
                out.push((color / 3 + if color % 3 == 2 { 1 } else { 0 }) as c_int);
            }

            if let Some(alpha) = reqs.alpha_bits {
                out.push(ffi::egl::ALPHA_SIZE as c_int);
                out.push(alpha as c_int);
            }

            if let Some(depth) = reqs.depth_bits {
                out.push(ffi::egl::DEPTH_SIZE as c_int);
                out.push(depth as c_int);
            }

            if let Some(stencil) = reqs.stencil_bits {
                out.push(ffi::egl::STENCIL_SIZE as c_int);
                out.push(stencil as c_int);
            }

            if let Some(true) = reqs.double_buffer {
                return Err(CreationError::NoAvailablePixelFormat);
            }

            if let Some(multisampling) = reqs.multisampling {
                out.push(ffi::egl::SAMPLES as c_int);
                out.push(multisampling as c_int);
            }

            if reqs.stereoscopy {
                return Err(CreationError::NoAvailablePixelFormat);
            }

            out.push(ffi::egl::NONE as c_int);
            out
        };

        // calling `eglChooseConfig`
        let mut config_id = mem::uninitialized();
        let mut num_configs = mem::uninitialized();
        if egl.ChooseConfig(display,
                            descriptor.as_ptr(),
                            &mut config_id,
                            1,
                            &mut num_configs) == 0 {
            return Err(CreationError::OsError(String::from("eglChooseConfig failed")));
        }
        if num_configs == 0 {
            return Err(CreationError::NoAvailablePixelFormat);
        }

        // analyzing each config
        macro_rules! attrib {
            ($egl:expr, $display:expr, $config:expr, $attr:expr) => (
                {
                    let mut value = mem::uninitialized();
                    let res = $egl.GetConfigAttrib($display, $config,
                                                   $attr as ffi::egl::types::EGLint, &mut value);
                    if res == 0 {
                        return Err(CreationError::OsError(String::from("eglGetConfigAttrib failed")));
                    }
                    value
                }
            )
        };

        let desc = PixelFormat {
            hardware_accelerated: attrib!(egl, display, config_id, ffi::egl::CONFIG_CAVEAT) !=
                                  ffi::egl::SLOW_CONFIG as i32,
            color_bits: attrib!(egl, display, config_id, ffi::egl::RED_SIZE) as u8 +
                        attrib!(egl, display, config_id, ffi::egl::BLUE_SIZE) as u8 +
                        attrib!(egl, display, config_id, ffi::egl::GREEN_SIZE) as u8,
            alpha_bits: attrib!(egl, display, config_id, ffi::egl::ALPHA_SIZE) as u8,
            depth_bits: attrib!(egl, display, config_id, ffi::egl::DEPTH_SIZE) as u8,
            stencil_bits: attrib!(egl, display, config_id, ffi::egl::STENCIL_SIZE) as u8,
            stereoscopy: false,
            double_buffer: true,
            multisampling: match attrib!(egl, display, config_id, ffi::egl::SAMPLES) {
                0 | 1 => None,
                a => Some(a as u16),
            },
            srgb: false, // TODO: use EGL_KHR_gl_colorspace to know that
        };

        let surface = {
            let surface = match native {
                Native::X11(_, window) |
                Native::Wayland(_, window) |
                Native::Gbm(_, window) => egl.CreateWindowSurface(display, config_id, window, ptr::null()),
            };

            if surface.is_null() {
                return Err(CreationError::OsError(String::from("eglCreateWindowSurface failed")));
            }
            surface
        };

        let mut context_attributes = Vec::with_capacity(10);
        let mut flags = 0;

        if egl_version >= (1, 5) || extensions.iter().any(|s| s == &"EGL_KHR_create_context") {
            context_attributes.push(ffi::egl::CONTEXT_MAJOR_VERSION as i32);
            context_attributes.push(version.0 as i32);
            context_attributes.push(ffi::egl::CONTEXT_MINOR_VERSION as i32);
            context_attributes.push(version.1 as i32);

            // handling robustness
            let supports_robustness = egl_version >= (1, 5) ||
                                      extensions
                                          .iter()
                                          .any(|s| s == "EGL_EXT_create_context_robustness");

            match attributes.robustness {
                Robustness::NotRobust => (),

                Robustness::NoError => {
                    if extensions
                           .iter()
                           .any(|s| s == "EGL_KHR_create_context_no_error") {
                        context_attributes.push(ffi::egl::CONTEXT_OPENGL_NO_ERROR_KHR as c_int);
                        context_attributes.push(1);
                    }
                }

                Robustness::RobustNoResetNotification => {
                    if supports_robustness {
                        context_attributes
                            .push(ffi::egl::CONTEXT_OPENGL_RESET_NOTIFICATION_STRATEGY as c_int);
                        context_attributes.push(ffi::egl::NO_RESET_NOTIFICATION as c_int);
                        flags |= ffi::egl::CONTEXT_OPENGL_ROBUST_ACCESS as c_int;
                    } else {
                        return Err(CreationError::RobustnessNotSupported);
                    }
                }

                Robustness::TryRobustNoResetNotification => {
                    if supports_robustness {
                        context_attributes
                            .push(ffi::egl::CONTEXT_OPENGL_RESET_NOTIFICATION_STRATEGY as c_int);
                        context_attributes.push(ffi::egl::NO_RESET_NOTIFICATION as c_int);
                        flags |= ffi::egl::CONTEXT_OPENGL_ROBUST_ACCESS as c_int;
                    }
                }

                Robustness::RobustLoseContextOnReset => {
                    if supports_robustness {
                        context_attributes
                            .push(ffi::egl::CONTEXT_OPENGL_RESET_NOTIFICATION_STRATEGY as c_int);
                        context_attributes.push(ffi::egl::LOSE_CONTEXT_ON_RESET as c_int);
                        flags |= ffi::egl::CONTEXT_OPENGL_ROBUST_ACCESS as c_int;
                    } else {
                        return Err(CreationError::RobustnessNotSupported);
                    }
                }

                Robustness::TryRobustLoseContextOnReset => {
                    if supports_robustness {
                        context_attributes
                            .push(ffi::egl::CONTEXT_OPENGL_RESET_NOTIFICATION_STRATEGY as c_int);
                        context_attributes.push(ffi::egl::LOSE_CONTEXT_ON_RESET as c_int);
                        flags |= ffi::egl::CONTEXT_OPENGL_ROBUST_ACCESS as c_int;
                    }
                }
            }

            if attributes.debug && egl_version >= (1, 5) {
                context_attributes.push(ffi::egl::CONTEXT_OPENGL_DEBUG as i32);
                context_attributes.push(ffi::egl::TRUE as i32);
            }

            context_attributes.push(ffi::egl::CONTEXT_FLAGS_KHR as i32);
            context_attributes.push(flags);

        } else if egl_version >= (1, 3) {
            // robustness is not supported
            match attributes.robustness {
                Robustness::RobustNoResetNotification |
                Robustness::RobustLoseContextOnReset => {
                    return Err(CreationError::RobustnessNotSupported);
                }
                _ => (),
            }

            context_attributes.push(ffi::egl::CONTEXT_CLIENT_VERSION as i32);
            context_attributes.push(version.0 as i32);
        }

        context_attributes.push(ffi::egl::NONE as i32);

        let context = egl.CreateContext(display, config_id, ptr::null(), context_attributes.as_ptr());

        if context.is_null() {
            match egl.GetError() as u32 {
                ffi::egl::BAD_ATTRIBUTE => return Err(CreationError::OpenGlVersionNotSupported),
                e => panic!("eglCreateContext failed: 0x{:x}", e),
            }
        }

        Ok(EGLContext {
               context: context as *const _,
               display: display as *const _,
               egl: egl,
               surface: surface as *const _,
               pixel_format: desc,
           })
    }

    /// Swaps buffers at the end of a frame.
    pub fn swap_buffers(&self) -> Result<(), SwapBuffersError> {
        let ret = unsafe {
            self.egl
                .SwapBuffers(self.display as *const _, self.surface as *const _)
        };

        if ret == 0 {
            match unsafe { self.egl.GetError() } as u32 {
                ffi::egl::CONTEXT_LOST => Err(SwapBuffersError::ContextLost),
                err => panic!("eglSwapBuffers failed (eglGetError returned 0x{:x})", err),
            }
        } else {
            Ok(())
        }
    }

    /// Returns the address of an OpenGL function.
    ///
    /// Supposes that the context has been made current before this function is called.
    pub unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void {
        let addr = CString::new(symbol.as_bytes()).unwrap();
        let addr = addr.as_ptr();
        self.egl.GetProcAddress(addr) as *const _
    }

    /// Returns true if the OpenGL context is the current one in the thread.
    pub fn is_current(&self) -> bool {
        unsafe { self.egl.GetCurrentContext() == self.context as *const _ }
    }

    /// Makes the OpenGL context the current context in the current thread.
    ///
    /// # Unsafety
    ///
    /// This function is marked unsafe, because the context cannot be made current
    /// on multiple threads.
    pub unsafe fn make_current(&self) -> Result<(), SwapBuffersError> {
        let ret = self.egl
            .MakeCurrent(self.display as *const _,
                         self.surface as *const _,
                         self.surface as *const _,
                         self.context as *const _);

        if ret == 0 {
            match self.egl.GetError() as u32 {
                ffi::egl::CONTEXT_LOST => Err(SwapBuffersError::ContextLost),
                err => panic!("eglMakeCurrent failed (eglGetError returned 0x{:x})", err),
            }
        } else {
            Ok(())
        }
    }

    /// Returns the pixel format of the main framebuffer of the context.
    pub fn get_pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }
}

unsafe impl Send for EGLContext {}
unsafe impl Sync for EGLContext {}

impl Drop for EGLContext {
    fn drop(&mut self) {
        unsafe {
            // we don't call MakeCurrent(0, 0) because we are not sure that the context
            // is still the current one
            self.egl
                .DestroyContext(self.display as *const _, self.context as *const _);
            self.egl
                .DestroySurface(self.display as *const _, self.surface as *const _);
            self.egl.Terminate(self.display as *const _);
        }
    }
}

/// Error that can happen when swapping buffers.
#[derive(Debug, Clone)]
pub enum SwapBuffersError {
    /// The OpenGL context has been lost and needs to be recreated.
    ///
    /// All the objects associated to it (textures, buffers, programs, etc.)
    /// need to be recreated from scratch.
    ///
    /// Operations will have no effect. Functions that read textures, buffers, etc.
    /// from OpenGL will return uninitialized data instead.
    ///
    /// A context loss usually happens on mobile devices when the user puts the
    /// application on sleep and wakes it up later. However any OpenGL implementation
    /// can theoretically lose the context at any time.
    ContextLost,
    /// The buffers have already been swapped.
    ///
    /// This error can be returned when `swap_buffers` has been called multiple times
    /// without any modification in between.
    AlreadySwapped,
}

/// Attributes to use when creating an OpenGL context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlAttributes {
    /// Describes the OpenGL API and version that are being requested when a context is created.
    ///
    /// `Some(3, 0)` will request a OpenGL ES 3.0 context for example.
    /// `None` means "don't care" (minimum will be 2.0).
    pub version: Option<(u8, u8)>,
    /// OpenGL profile to use
    pub profile: Option<GlProfile>,
    /// Whether to enable the debug flag of the context.
    ///
    /// Debug contexts are usually slower but give better error reporting.
    pub debug: bool,
    /// How the OpenGL context should detect errors.
    pub robustness: Robustness,
    /// Whether to use vsync. If vsync is enabled, calling swap_buffers will block until the screen refreshes.
    /// This is typically used to prevent screen tearing.
    pub vsync: bool,
}

/// Specifies the tolerance of the OpenGL context to faults. If you accept raw OpenGL commands and/or raw
/// shader code from an untrusted source, you should definitely care about this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Robustness {
    /// Not everything is checked. Your application can crash if you do something wrong with your shaders.
    NotRobust,
    /// The driver doesn't check anything. This option is very dangerous. Please know what you're doing before
    /// using it. See the GL_KHR_no_error extension.
    ///
    /// Since this option is purely an optimisation, no error will be returned if the backend doesn't support it.
    /// Instead it will automatically fall back to NotRobust.
    NoError,
    /// Everything is checked to avoid any crash. The driver will attempt to avoid any problem, but if a problem occurs
    /// the behavior is implementation-defined. You are just guaranteed not to get a crash.
    RobustNoResetNotification,
    /// Same as RobustNoResetNotification but the context creation doesn't fail if it's not supported.
    TryRobustNoResetNotification,
    /// Everything is checked to avoid any crash. If a problem occurs, the context will enter a "context lost" state.
    /// It must then be recreated.
    RobustLoseContextOnReset,
    /// Same as RobustLoseContextOnReset but the context creation doesn't fail if it's not supported.
    TryRobustLoseContextOnReset,
}

/// Describes the requested OpenGL context profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlProfile {
    /// Include all the immediate more functions and definitions.
    Compatibility,
    /// Include all the future-compatible functions and definitions.
    Core,
}

/// Describes how the backend should choose a pixel format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PixelFormatRequirements {
    /// If `true`, only hardware-accelerated formats will be conisdered. If `false`, only software renderers.
    /// `None` means "don't care". Default is `None`.
    pub hardware_accelerated: Option<bool>,
    /// Minimum number of bits for the color buffer, excluding alpha. None means "don't care". The default is `None``.
    pub color_bits: Option<u8>,
    /// If `true`, the color buffer must be in a floating point format. Default is `false`.
    ///
    /// Using floating points allows you to write values outside of the `[0.0, 1.0]` range.
    pub float_color_buffer: bool,
    /// Minimum number of bits for the alpha in the color buffer. `None` means "don't care". The default is `None`.
    pub alpha_bits: Option<u8>,
    /// Minimum number of bits for the depth buffer. `None` means "don't care". The default value is `None`.
    pub depth_bits: Option<u8>,
    /// Minimum number of bits for the depth buffer. `None` means "don't care". The default value is `None`.
    pub stencil_bits: Option<u8>,
    /// If `true`, only double-buffered formats will be considered. If `false`, only single-buffer formats.
    /// `None` means "don't care". The default is `None`.
    pub double_buffer: Option<bool>,
    /// Contains the minimum number of samples per pixel in the color, depth and stencil buffers.
    /// `None` means "don't care". Default is `None`. A value of `Some(0)` indicates that multisampling must not be enabled.
    pub multisampling: Option<u16>,
    /// If `true`, only stereoscopic formats will be considered. If `false`, only non-stereoscopic formats.
    /// The default is `false`.
    pub stereoscopy: bool,
}

/// Describes the pixel format of the main framebuffer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelFormat {
    /// is the format hardware accelerated
    pub hardware_accelerated: bool,
    /// number of bits used for colors
    pub color_bits: u8,
    /// number of bits used for alpha channel
    pub alpha_bits: u8,
    /// number of bits used for depth channel
    pub depth_bits: u8,
    /// number of bits used for stencil buffer
    pub stencil_bits: u8,
    /// is stereoscopy enabled
    pub stereoscopy: bool,
    /// is double buffering enabled
    pub double_buffer: bool,
    /// number of samples used for multisampling if enabled
    pub multisampling: Option<u16>,
    /// is srgb enabled
    pub srgb: bool,
}

/// Trait that describes objects that have an OpenGl context
/// and can be used to render upon
pub trait EGLGraphicsBackend: GraphicsBackend {
    /// Swaps buffers at the end of a frame.
    fn swap_buffers(&self) -> Result<(), SwapBuffersError>;

    /// Returns the address of an OpenGL function.
    ///
    /// Supposes that the context has been made current before this function is called.
    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void;

    /// Returns the dimensions of the window, or screen, etc in points.
    ///
    /// That are the scaled pixels of the underlying graphics backend.
    /// For nested compositors this will respect the scaling of the root compositor.
    /// For drawing directly onto hardware this unit will be equal to actual pixels.
    fn get_framebuffer_dimensions(&self) -> (u32, u32);

    /// Returns true if the OpenGL context is the current one in the thread.
    fn is_current(&self) -> bool;

    /// Makes the OpenGL context the current context in the current thread.
    ///
    /// # Unsafety
    ///
    /// This function is marked unsafe, because the context cannot be made current
    /// on multiple threads.
    unsafe fn make_current(&self) -> Result<(), SwapBuffersError>;

    /// Returns the pixel format of the main framebuffer of the context.
    fn get_pixel_format(&self) -> PixelFormat;
}
