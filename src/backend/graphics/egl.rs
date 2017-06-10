//! Common traits and types for egl context creation and rendering

/// Large parts of the following file are taken from
/// https://github.com/tomaka/glutin/tree/044e651edf67a2029eecc650dd42546af1501414/src/api/egl/
///
/// It therefore falls under glutin's Apache 2.0 license
/// (see https://github.com/tomaka/glutin/tree/044e651edf67a2029eecc650dd42546af1501414/LICENSE)

use super::GraphicsBackend;

use libloading::Library;
use nix::{c_int, c_void};
use slog;
use std::error::{self, Error};

use std::ffi::{CStr, CString};
use std::fmt;
use std::io;
use std::mem;
use std::ptr;
use std::ops::Deref;

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
pub enum NativeDisplay {
    /// X11 Display to create an `EGLContext` upon.
    X11(ffi::NativeDisplayType),
    /// Wayland Display to create an `EGLContext` upon.
    Wayland(ffi::NativeDisplayType),
    /// GBM Display
    Gbm(ffi::NativeDisplayType),
}

/// Native types to create an `EGLSurface` from.
/// Currently supported providers are X11, Wayland and GBM.
#[derive(Clone, Copy)]
pub enum NativeSurface {
    /// X11 Window to create an `EGLSurface` upon.
    X11(ffi::NativeWindowType),
    /// Wayland Surface to create an `EGLSurface` upon.
    Wayland(ffi::NativeWindowType),
    /// GBM Surface
    Gbm(ffi::NativeWindowType),
}

/// Error that can happen while creating an `EGLContext` or `EGLSurface`
#[derive(Debug)]
pub enum CreationError {
    /// I/O error from the underlying system
    IoError(io::Error),
    /// Operating System error
    OsError(String),
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
    _lib: Library,
    context: ffi::egl::types::EGLContext,
    display: ffi::egl::types::EGLDisplay,
    egl: ffi::egl::Egl,
    config_id: ffi::egl::types::EGLConfig,
    surface_attributes: Vec<c_int>,
    pixel_format: PixelFormat,
    logger: slog::Logger,
}

/// EGL surface of a given egl context for rendering
pub struct EGLSurface<'a> {
    context: &'a EGLContext,
    surface: ffi::egl::types::EGLSurface,
}

impl<'a> Deref for EGLSurface<'a> {
    type Target = EGLContext;
    fn deref(&self) -> &Self::Target {
        self.context
    }
}

impl EGLContext {
    /// Create a new EGL context
    ///
    /// # Unsafety
    ///
    /// This method is marked unsafe, because the contents of `NativeDisplay` cannot be verified and msy
    /// contain dangeling pointers are similar unsafe content
    pub unsafe fn new<L>(native: NativeDisplay, mut attributes: GlAttributes, reqs: PixelFormatRequirements,
                         logger: L)
                         -> Result<EGLContext, CreationError>
        where L: Into<Option<::slog::Logger>>
    {
        let logger = logger.into();
        let log = ::slog_or_stdlog(logger.clone()).new(o!("smithay_module" => "renderer_egl"));
        trace!(log, "Loading libEGL");

        // If no version is given, try OpenGLES 3.0, if available,
        // fallback to 2.0 otherwise
        let version = match attributes.version {
            Some((3, x)) => (3, x),
            Some((2, x)) => (2, x),
            None => {
                debug!(log, "Trying to initialize EGL with OpenGLES 3.0");
                attributes.version = Some((3, 0));
                match EGLContext::new(native, attributes, reqs, logger.clone()) {
                    Ok(x) => return Ok(x),
                    Err(err) => {
                        warn!(log, "EGL OpenGLES 3.0 Initialization failed with {}", err);
                        debug!(log, "Trying to initialize EGL with OpenGLES 2.0");
                        attributes.version = Some((2, 0));
                        return EGLContext::new(native, attributes, reqs, logger);
                    }
                }
            }
            Some((1, _)) => {
                error!(log,
                       "OpenGLES 1.* is not supported by the EGL renderer backend");
                return Err(CreationError::OpenGlVersionNotSupported);
            }
            Some(version) => {
                error!(log,
                       "OpenGLES {:?} is unknown and not supported by the EGL renderer backend",
                       version);
                return Err(CreationError::OpenGlVersionNotSupported);
            }
        };

        let lib = Library::new("libEGL.so.1")?;
        let egl = ffi::egl::Egl::load_with(|sym| {
                                               let name = CString::new(sym).unwrap();
                                               let symbol = lib.get::<*mut c_void>(name.as_bytes());
                                               match symbol {
                                                   Ok(x) => *x as *const _,
                                                   Err(_) => ptr::null(),
                                               }
                                           });

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

        debug!(log, "EGL No-Display Extensions: {:?}", dp_extensions);

        let has_dp_extension = |e: &str| dp_extensions.iter().any(|s| s == e);

        let display = match native {
            NativeDisplay::X11(display) if has_dp_extension("EGL_KHR_platform_x11") &&
                                       egl.GetPlatformDisplay.is_loaded() => {
                trace!(log, "EGL Display Initialization via EGL_KHR_platform_x11");
                egl.GetPlatformDisplay(ffi::egl::PLATFORM_X11_KHR, display as *mut _, ptr::null())
            }

            NativeDisplay::X11(display) if has_dp_extension("EGL_EXT_platform_x11") &&
                                       egl.GetPlatformDisplayEXT.is_loaded() => {
                trace!(log, "EGL Display Initialization via EGL_EXT_platform_x11");
                egl.GetPlatformDisplayEXT(ffi::egl::PLATFORM_X11_EXT, display as *mut _, ptr::null())
            }

            NativeDisplay::Gbm(display) if has_dp_extension("EGL_KHR_platform_gbm") &&
                                       egl.GetPlatformDisplay.is_loaded() => {
                trace!(log, "EGL Display Initialization via EGL_KHR_platform_gbm");
                egl.GetPlatformDisplay(ffi::egl::PLATFORM_GBM_KHR, display as *mut _, ptr::null())
            }

            NativeDisplay::Gbm(display) if has_dp_extension("EGL_MESA_platform_gbm") &&
                                       egl.GetPlatformDisplayEXT.is_loaded() => {
                trace!(log, "EGL Display Initialization via EGL_MESA_platform_gbm");
                egl.GetPlatformDisplayEXT(ffi::egl::PLATFORM_GBM_KHR, display as *mut _, ptr::null())
            }

            NativeDisplay::Wayland(display) if has_dp_extension("EGL_KHR_platform_wayland") &&
                                           egl.GetPlatformDisplay.is_loaded() => {
                trace!(log,
                       "EGL Display Initialization via EGL_KHR_platform_wayland");
                egl.GetPlatformDisplay(ffi::egl::PLATFORM_WAYLAND_KHR,
                                       display as *mut _,
                                       ptr::null())
            }

            NativeDisplay::Wayland(display) if has_dp_extension("EGL_EXT_platform_wayland") &&
                                           egl.GetPlatformDisplayEXT.is_loaded() => {
                trace!(log,
                       "EGL Display Initialization via EGL_EXT_platform_wayland");
                egl.GetPlatformDisplayEXT(ffi::egl::PLATFORM_WAYLAND_EXT,
                                          display as *mut _,
                                          ptr::null())
            }

            NativeDisplay::X11(display) |
            NativeDisplay::Gbm(display) |
            NativeDisplay::Wayland(display) => {
                trace!(log, "Default EGL Display Initialization via GetDisplay");
                egl.GetDisplay(display as *mut _)
            }
        };

        let egl_version = {
            let mut major: ffi::egl::types::EGLint = mem::uninitialized();
            let mut minor: ffi::egl::types::EGLint = mem::uninitialized();

            if egl.Initialize(display, &mut major, &mut minor) == 0 {
                return Err(CreationError::OsError(String::from("eglInitialize failed")));
            }

            info!(log, "EGL Initialized");
            info!(log, "EGL Version: {:?}", (major, minor));

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

        info!(log, "EGL Extensions: {:?}", extensions);

        if egl_version >= (1, 2) && egl.BindAPI(ffi::egl::OPENGL_ES_API) == 0 {
            error!(log,
                   "OpenGLES not supported by the underlying EGL implementation");
            return Err(CreationError::OpenGlVersionNotSupported);
        }

        let descriptor = {
            let mut out: Vec<c_int> = Vec::with_capacity(37);

            if egl_version >= (1, 2) {
                trace!(log, "Setting COLOR_BUFFER_TYPE to RGB_BUFFER");
                out.push(ffi::egl::COLOR_BUFFER_TYPE as c_int);
                out.push(ffi::egl::RGB_BUFFER as c_int);
            }

            trace!(log, "Setting SURFACE_TYPE to WINDOW");

            out.push(ffi::egl::SURFACE_TYPE as c_int);
            // TODO: Some versions of Mesa report a BAD_ATTRIBUTE error
            // if we ask for PBUFFER_BIT as well as WINDOW_BIT
            out.push((ffi::egl::WINDOW_BIT) as c_int);

            match version {
                (3, _) => {
                    if egl_version < (1, 3) {
                        error!(log,
                               "OpenglES 3.* is not supported on EGL Versions lower then 1.3");
                        return Err(CreationError::NoAvailablePixelFormat);
                    }
                    trace!(log, "Setting RENDERABLE_TYPE to OPENGL_ES3");
                    out.push(ffi::egl::RENDERABLE_TYPE as c_int);
                    out.push(ffi::egl::OPENGL_ES3_BIT as c_int);
                    trace!(log, "Setting CONFORMANT to OPENGL_ES3");
                    out.push(ffi::egl::CONFORMANT as c_int);
                    out.push(ffi::egl::OPENGL_ES3_BIT as c_int);
                }
                (2, _) => {
                    if egl_version < (1, 3) {
                        error!(log,
                               "OpenglES 2.* is not supported on EGL Versions lower then 1.3");
                        return Err(CreationError::NoAvailablePixelFormat);
                    }
                    trace!(log, "Setting RENDERABLE_TYPE to OPENGL_ES2");
                    out.push(ffi::egl::RENDERABLE_TYPE as c_int);
                    out.push(ffi::egl::OPENGL_ES2_BIT as c_int);
                    trace!(log, "Setting CONFORMANT to OPENGL_ES2");
                    out.push(ffi::egl::CONFORMANT as c_int);
                    out.push(ffi::egl::OPENGL_ES2_BIT as c_int);
                }
                (_, _) => unreachable!(),
            };

            if let Some(hardware_accelerated) = reqs.hardware_accelerated {
                out.push(ffi::egl::CONFIG_CAVEAT as c_int);
                out.push(if hardware_accelerated {
                             trace!(log, "Setting CONFIG_CAVEAT to NONE");
                             ffi::egl::NONE as c_int
                         } else {
                             trace!(log, "Setting CONFIG_CAVEAT to SLOW_CONFIG");
                             ffi::egl::SLOW_CONFIG as c_int
                         });
            }

            if let Some(color) = reqs.color_bits {
                trace!(log, "Setting RED_SIZE to {}", color / 3);
                out.push(ffi::egl::RED_SIZE as c_int);
                out.push((color / 3) as c_int);
                trace!(log,
                       "Setting GREEN_SIZE to {}",
                       color / 3 + if color % 3 != 0 { 1 } else { 0 });
                out.push(ffi::egl::GREEN_SIZE as c_int);
                out.push((color / 3 + if color % 3 != 0 { 1 } else { 0 }) as c_int);
                trace!(log,
                       "Setting BLUE_SIZE to {}",
                       color / 3 + if color % 3 == 2 { 1 } else { 0 });
                out.push(ffi::egl::BLUE_SIZE as c_int);
                out.push((color / 3 + if color % 3 == 2 { 1 } else { 0 }) as c_int);
            }

            if let Some(alpha) = reqs.alpha_bits {
                trace!(log, "Setting ALPHA_SIZE to {}", alpha);
                out.push(ffi::egl::ALPHA_SIZE as c_int);
                out.push(alpha as c_int);
            }

            if let Some(depth) = reqs.depth_bits {
                trace!(log, "Setting DEPTH_SIZE to {}", depth);
                out.push(ffi::egl::DEPTH_SIZE as c_int);
                out.push(depth as c_int);
            }

            if let Some(stencil) = reqs.stencil_bits {
                trace!(log, "Setting STENCIL_SIZE to {}", stencil);
                out.push(ffi::egl::STENCIL_SIZE as c_int);
                out.push(stencil as c_int);
            }

            if let Some(multisampling) = reqs.multisampling {
                trace!(log, "Setting SAMPLES to {}", multisampling);
                out.push(ffi::egl::SAMPLES as c_int);
                out.push(multisampling as c_int);
            }

            if reqs.stereoscopy {
                error!(log, "Stereoscopy is currently unsupported (sorry!)");
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
            error!(log, "No matching color format found");
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

        info!(log, "Selected color format: {:?}", desc);

        let mut context_attributes = Vec::with_capacity(10);

        if egl_version >= (1, 5) || extensions.iter().any(|s| s == &"EGL_KHR_create_context") {
            trace!(log, "Setting CONTEXT_MAJOR_VERSION to {}", version.0);
            context_attributes.push(ffi::egl::CONTEXT_MAJOR_VERSION as i32);
            context_attributes.push(version.0 as i32);
            trace!(log, "Setting CONTEXT_MINOR_VERSION to {}", version.1);
            context_attributes.push(ffi::egl::CONTEXT_MINOR_VERSION as i32);
            context_attributes.push(version.1 as i32);

            if attributes.debug && egl_version >= (1, 5) {
                trace!(log, "Setting CONTEXT_OPENGL_DEBUG to TRUE");
                context_attributes.push(ffi::egl::CONTEXT_OPENGL_DEBUG as i32);
                context_attributes.push(ffi::egl::TRUE as i32);
            }

            context_attributes.push(ffi::egl::CONTEXT_FLAGS_KHR as i32);
            context_attributes.push(0);

        } else if egl_version >= (1, 3) {
            trace!(log, "Setting CONTEXT_CLIENT_VERSION to {}", version.0);
            context_attributes.push(ffi::egl::CONTEXT_CLIENT_VERSION as i32);
            context_attributes.push(version.0 as i32);
        }

        context_attributes.push(ffi::egl::NONE as i32);

        trace!(log, "Creating EGL context...");
        let context = egl.CreateContext(display, config_id, ptr::null(), context_attributes.as_ptr());

        if context.is_null() {
            match egl.GetError() as u32 {
                ffi::egl::BAD_ATTRIBUTE => {
                    error!(log,
                           "Context creation failed as one or more requirements could not be met. Try removing some gl attributes or pixel format requirements");
                    return Err(CreationError::OpenGlVersionNotSupported);
                }
                e => panic!("eglCreateContext failed: 0x{:x}", e),
            }
        }
        debug!(log, "EGL context successfully created");

        let surface_attributes = {
            let mut out: Vec<c_int> = Vec::with_capacity(2);

            match reqs.double_buffer {
                Some(true) => {
                    trace!(log, "Setting RENDER_BUFFER to BACK_BUFFER");
                    out.push(ffi::egl::RENDER_BUFFER as c_int);
                    out.push(ffi::egl::BACK_BUFFER as c_int);
                }
                Some(false) => {
                    trace!(log, "Setting RENDER_BUFFER to SINGLE_BUFFER");
                    out.push(ffi::egl::RENDER_BUFFER as c_int);
                    out.push(ffi::egl::SINGLE_BUFFER as c_int);
                }
                None => {}
            }

            out
        };

        info!(log, "EGL context created");

        Ok(EGLContext {
            _lib: lib,
            context: context as *const _,
            display: display as *const _,
            egl: egl,
            config_id: config_id,
            surface_attributes: surface_attributes,
            pixel_format: desc,
            logger: log,
        })
    }

    /// Creates a surface bound to the given egl context for rendering
    ///
    /// # Unsafety
    ///
    /// This method is marked unsafe, because the contents of `NativeSurface` cannot be verified and msy
    /// contain dangeling pointers are similar unsafe content
    pub unsafe fn create_surface<'a>(&'a self, native: NativeSurface) -> Result<EGLSurface<'a>, CreationError> {
        trace!(self.logger, "Creating EGL window surface...");

        let surface = {
            let surface = match native {
                NativeSurface::X11(window) |
                NativeSurface::Wayland(window) |
                NativeSurface::Gbm(window) => self.egl.CreateWindowSurface(self.display, self.config_id, window, self.surface_attributes.as_ptr()),
            };

            if surface.is_null() {
                return Err(CreationError::OsError(String::from("eglCreateWindowSurface failed")));
            }

            surface
        };

        debug!(self.logger, "EGL window surface successfully created");

        Ok(EGLSurface {
            context: &self,
            surface: surface,
        })
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

    /// Returns the pixel format of the main framebuffer of the context.
    pub fn get_pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }
}

impl<'a> EGLSurface<'a> {
    /// Swaps buffers at the end of a frame.
    pub fn swap_buffers(&self) -> Result<(), SwapBuffersError> {
        let ret = unsafe {
            self.context.egl
                .SwapBuffers(self.context.display as *const _, self.surface as *const _)
        };

        if ret == 0 {
            match unsafe { self.context.egl.GetError() } as u32 {
                ffi::egl::CONTEXT_LOST => Err(SwapBuffersError::ContextLost),
                err => panic!("eglSwapBuffers failed (eglGetError returned 0x{:x})", err),
            }
        } else {
            Ok(())
        }
    }

    /// Makes the OpenGL context the current context in the current thread.
    ///
    /// # Unsafety
    ///
    /// This function is marked unsafe, because the context cannot be made current
    /// on multiple threads.
    pub unsafe fn make_current(&self) -> Result<(), SwapBuffersError> {
        let ret = self.context.egl
            .MakeCurrent(self.context.display as *const _,
                         self.surface as *const _,
                         self.surface as *const _,
                         self.context.context as *const _);

        if ret == 0 {
            match self.context.egl.GetError() as u32 {
                ffi::egl::CONTEXT_LOST => Err(SwapBuffersError::ContextLost),
                err => panic!("eglMakeCurrent failed (eglGetError returned 0x{:x})", err),
            }
        } else {
            Ok(())
        }
    }
}

unsafe impl Send for EGLContext {}
unsafe impl Sync for EGLContext {}
unsafe impl<'a> Send for EGLSurface<'a> {}
unsafe impl<'a> Sync for EGLSurface<'a> {}

impl Drop for EGLContext {
    fn drop(&mut self) {
        unsafe {
            // we don't call MakeCurrent(0, 0) because we are not sure that the context
            // is still the current one
            self.egl
                .DestroyContext(self.display as *const _, self.context as *const _);
            self.egl.Terminate(self.display as *const _);
        }
    }
}

impl<'a> Drop for EGLSurface<'a> {
    fn drop(&mut self) {
        unsafe {
            self.context.egl
                .DestroySurface(self.context.display as *const _, self.surface as *const _);
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
    /// Whether to use vsync. If vsync is enabled, calling swap_buffers will block until the screen refreshes.
    /// This is typically used to prevent screen tearing.
    pub vsync: bool,
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
