//! EGL context related structs

use super::{error::*, ffi, native, EGLSurface};
use backend::graphics::PixelFormat;
use nix::libc::{c_int, c_void};
use slog;
use std::{
    cell::{Ref, RefCell, RefMut},
    ffi::{CStr, CString},
    marker::PhantomData,
    mem, ptr,
    rc::Rc,
};

/// EGL context for rendering
pub struct EGLContext<B: native::Backend, N: native::NativeDisplay<B>> {
    native: RefCell<N>,
    pub(crate) context: Rc<ffi::egl::types::EGLContext>,
    pub(crate) display: Rc<ffi::egl::types::EGLDisplay>,
    pub(crate) config_id: ffi::egl::types::EGLConfig,
    pub(crate) surface_attributes: Vec<c_int>,
    pixel_format: PixelFormat,
    pub(crate) wl_drm_support: bool,
    logger: slog::Logger,
    _backend: PhantomData<B>,
}

impl<B: native::Backend, N: native::NativeDisplay<B>> EGLContext<B, N> {
    /// Create a new `EGLContext` from a given `NativeDisplay`
    pub fn new<L>(
        native: N,
        attributes: GlAttributes,
        reqs: PixelFormatRequirements,
        logger: L,
    ) -> Result<EGLContext<B, N>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger.into()).new(o!("smithay_module" => "renderer_egl"));
        let ptr = native.ptr()?;
        let (context, display, config_id, surface_attributes, pixel_format, wl_drm_support) =
            unsafe { EGLContext::<B, N>::new_internal(ptr, attributes, reqs, log.clone()) }?;

        Ok(EGLContext {
            native: RefCell::new(native),
            context,
            display,
            config_id,
            surface_attributes,
            pixel_format,
            wl_drm_support,
            logger: log,
            _backend: PhantomData,
        })
    }

    unsafe fn new_internal(
        ptr: ffi::NativeDisplayType,
        mut attributes: GlAttributes,
        reqs: PixelFormatRequirements,
        log: ::slog::Logger,
    ) -> Result<(
        Rc<ffi::egl::types::EGLContext>,
        Rc<ffi::egl::types::EGLDisplay>,
        ffi::egl::types::EGLConfig,
        Vec<c_int>,
        PixelFormat,
        bool,
    )> {
        // If no version is given, try OpenGLES 3.0, if available,
        // fallback to 2.0 otherwise
        let version = match attributes.version {
            Some((3, x)) => (3, x),
            Some((2, x)) => (2, x),
            None => {
                debug!(log, "Trying to initialize EGL with OpenGLES 3.0");
                attributes.version = Some((3, 0));
                match EGLContext::<B, N>::new_internal(ptr, attributes, reqs, log.clone()) {
                    Ok(x) => return Ok(x),
                    Err(err) => {
                        warn!(log, "EGL OpenGLES 3.0 Initialization failed with {}", err);
                        debug!(log, "Trying to initialize EGL with OpenGLES 2.0");
                        attributes.version = Some((2, 0));
                        return EGLContext::<B, N>::new_internal(ptr, attributes, reqs, log);
                    }
                }
            }
            Some((1, x)) => {
                error!(log, "OpenGLES 1.* is not supported by the EGL renderer backend");
                bail!(ErrorKind::OpenGlVersionNotSupported((1, x)));
            }
            Some(version) => {
                error!(
                    log,
                    "OpenGLES {:?} is unknown and not supported by the EGL renderer backend", version
                );
                bail!(ErrorKind::OpenGlVersionNotSupported(version));
            }
        };

        ffi::egl::LOAD.call_once(|| {
            fn constrain<F>(f: F) -> F
            where
                F: for<'a> Fn(&'a str) -> *const ::std::os::raw::c_void,
            {
                f
            };

            ffi::egl::load_with(|sym| {
                let name = CString::new(sym).unwrap();
                let symbol = ffi::egl::LIB.get::<*mut c_void>(name.as_bytes());
                match symbol {
                    Ok(x) => *x as *const _,
                    Err(_) => ptr::null(),
                }
            });
            let proc_address = constrain(|sym| {
                let addr = CString::new(sym).unwrap();
                let addr = addr.as_ptr();
                ffi::egl::GetProcAddress(addr) as *const _
            });
            ffi::egl::load_with(&proc_address);
            ffi::egl::BindWaylandDisplayWL::load_with(&proc_address);
            ffi::egl::UnbindWaylandDisplayWL::load_with(&proc_address);
            ffi::egl::QueryWaylandBufferWL::load_with(&proc_address);
        });

        // the first step is to query the list of extensions without any display, if supported
        let dp_extensions = {
            let p = ffi::egl::QueryString(ffi::egl::NO_DISPLAY, ffi::egl::EXTENSIONS as i32);

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

        let display = B::get_display(ptr, |e: &str| dp_extensions.iter().any(|s| s == e), log.clone());
        if display == ffi::egl::NO_DISPLAY {
            error!(log, "EGL Display is not valid");
            bail!(ErrorKind::DisplayNotSupported);
        }

        let egl_version = {
            let mut major: ffi::egl::types::EGLint = mem::uninitialized();
            let mut minor: ffi::egl::types::EGLint = mem::uninitialized();

            if ffi::egl::Initialize(display, &mut major, &mut minor) == 0 {
                bail!(ErrorKind::InitFailed);
            }

            info!(log, "EGL Initialized");
            info!(log, "EGL Version: {:?}", (major, minor));

            (major, minor)
        };

        // the list of extensions supported by the client once initialized is different from the
        // list of extensions obtained earlier
        let extensions = if egl_version >= (1, 2) {
            let p = CStr::from_ptr(ffi::egl::QueryString(display, ffi::egl::EXTENSIONS as i32));
            let list = String::from_utf8(p.to_bytes().to_vec()).unwrap_or_else(|_| String::new());
            list.split(' ').map(|e| e.to_string()).collect::<Vec<_>>()
        } else {
            vec![]
        };

        info!(log, "EGL Extensions: {:?}", extensions);

        if egl_version >= (1, 2) && ffi::egl::BindAPI(ffi::egl::OPENGL_ES_API) == 0 {
            error!(log, "OpenGLES not supported by the underlying EGL implementation");
            bail!(ErrorKind::OpenGlesNotSupported);
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
                        error!(
                            log,
                            "OpenglES 3.* is not supported on EGL Versions lower then 1.3"
                        );
                        bail!(ErrorKind::NoAvailablePixelFormat);
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
                        error!(
                            log,
                            "OpenglES 2.* is not supported on EGL Versions lower then 1.3"
                        );
                        bail!(ErrorKind::NoAvailablePixelFormat);
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
                trace!(
                    log,
                    "Setting GREEN_SIZE to {}",
                    color / 3 + if color % 3 != 0 { 1 } else { 0 }
                );
                out.push(ffi::egl::GREEN_SIZE as c_int);
                out.push((color / 3 + if color % 3 != 0 { 1 } else { 0 }) as c_int);
                trace!(
                    log,
                    "Setting BLUE_SIZE to {}",
                    color / 3 + if color % 3 == 2 { 1 } else { 0 }
                );
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
                bail!(ErrorKind::NoAvailablePixelFormat);
            }

            out.push(ffi::egl::NONE as c_int);
            out
        };

        // calling `eglChooseConfig`
        let mut config_id = mem::uninitialized();
        let mut num_configs = mem::uninitialized();
        if ffi::egl::ChooseConfig(display, descriptor.as_ptr(), &mut config_id, 1, &mut num_configs) == 0 {
            bail!(ErrorKind::ConfigFailed);
        }
        if num_configs == 0 {
            error!(log, "No matching color format found");
            bail!(ErrorKind::NoAvailablePixelFormat);
        }

        // analyzing each config
        macro_rules! attrib {
            ($display:expr, $config:expr, $attr:expr) => {{
                let mut value = mem::uninitialized();
                let res = ffi::egl::GetConfigAttrib(
                    $display,
                    $config,
                    $attr as ffi::egl::types::EGLint,
                    &mut value,
                );
                if res == 0 {
                    bail!(ErrorKind::ConfigFailed);
                }
                value
            }};
        };

        let desc = PixelFormat {
            hardware_accelerated: attrib!(display, config_id, ffi::egl::CONFIG_CAVEAT)
                != ffi::egl::SLOW_CONFIG as i32,
            color_bits: attrib!(display, config_id, ffi::egl::RED_SIZE) as u8
                + attrib!(display, config_id, ffi::egl::BLUE_SIZE) as u8
                + attrib!(display, config_id, ffi::egl::GREEN_SIZE) as u8,
            alpha_bits: attrib!(display, config_id, ffi::egl::ALPHA_SIZE) as u8,
            depth_bits: attrib!(display, config_id, ffi::egl::DEPTH_SIZE) as u8,
            stencil_bits: attrib!(display, config_id, ffi::egl::STENCIL_SIZE) as u8,
            stereoscopy: false,
            double_buffer: true,
            multisampling: match attrib!(display, config_id, ffi::egl::SAMPLES) {
                0 | 1 => None,
                a => Some(a as u16),
            },
            srgb: false, // TODO: use EGL_KHR_gl_colorspace to know that
        };

        info!(log, "Selected color format: {:?}", desc);

        let mut context_attributes = Vec::with_capacity(10);

        if egl_version >= (1, 5) || extensions.iter().any(|s| *s == "EGL_KHR_create_context") {
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
        let context = ffi::egl::CreateContext(display, config_id, ptr::null(), context_attributes.as_ptr());

        if context.is_null() {
            match ffi::egl::GetError() as u32 {
                ffi::egl::BAD_ATTRIBUTE => bail!(ErrorKind::CreationFailed),
                err_no => bail!(ErrorKind::Unknown(err_no)),
            }
        }
        debug!(log, "EGL context successfully created");

        let surface_attributes = {
            let mut out: Vec<c_int> = Vec::with_capacity(3);

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

            out.push(ffi::egl::NONE as i32);
            out
        };

        info!(log, "EGL context created");

        // make current and get list of gl extensions
        ffi::egl::MakeCurrent(display as *const _, ptr::null(), ptr::null(), context as *const _);

        Ok((
            Rc::new(context as *const _),
            Rc::new(display as *const _),
            config_id,
            surface_attributes,
            desc,
            extensions.iter().any(|s| *s == "EGL_WL_bind_wayland_display"),
        ))
    }

    /// Creates a surface for rendering
    pub fn create_surface(&self, args: N::Arguments) -> Result<EGLSurface<B::Surface>> {
        trace!(self.logger, "Creating EGL window surface.");
        let surface = self
            .native
            .borrow_mut()
            .create_surface(args)
            .chain_err(|| ErrorKind::SurfaceCreationFailed)?;
        EGLSurface::new(self, surface).map(|x| {
            debug!(self.logger, "EGL surface successfully created");
            x
        })
    }

    /// Returns the address of an OpenGL function.
    ///
    /// Supposes that the context has been made current before this function is called.
    pub unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void {
        let addr = CString::new(symbol.as_bytes()).unwrap();
        let addr = addr.as_ptr();
        ffi::egl::GetProcAddress(addr) as *const _
    }

    /// Returns true if the OpenGL context is the current one in the thread.
    pub fn is_current(&self) -> bool {
        unsafe { ffi::egl::GetCurrentContext() == (*self.context) as *const _ }
    }

    /// Returns the pixel format of the main framebuffer of the context.
    pub fn get_pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    /// Borrow the underlying native display.
    ///
    /// This follows the same semantics as `std::cell:RefCell`.
    /// Multiple read-only borrows are possible. Borrowing the
    /// backend while there is a mutable reference will panic.
    pub fn borrow(&self) -> Ref<N> {
        self.native.borrow()
    }

    /// Borrow the underlying native display mutably.
    ///
    /// This follows the same semantics as `std::cell:RefCell`.
    /// Holding any other borrow while trying to borrow the backend
    /// mutably will panic. Note that EGL will borrow the display
    /// mutably during surface creation.
    pub fn borrow_mut(&self) -> RefMut<N> {
        self.native.borrow_mut()
    }
}

unsafe impl<B: native::Backend, N: native::NativeDisplay<B> + Send> Send for EGLContext<B, N> {}
unsafe impl<B: native::Backend, N: native::NativeDisplay<B> + Sync> Sync for EGLContext<B, N> {}

impl<B: native::Backend, N: native::NativeDisplay<B>> Drop for EGLContext<B, N> {
    fn drop(&mut self) {
        unsafe {
            // we don't call MakeCurrent(0, 0) because we are not sure that the context
            // is still the current one
            ffi::egl::DestroyContext((*self.display) as *const _, (*self.context) as *const _);
            ffi::egl::Terminate((*self.display) as *const _);
        }
    }
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
    /// Whether to use vsync. If vsync is enabled, calling `swap_buffers` will block until the screen refreshes.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelFormatRequirements {
    /// If `true`, only hardware-accelerated formats will be considered. If `false`, only software renderers.
    /// `None` means "don't care". Default is `None`.
    pub hardware_accelerated: Option<bool>,
    /// Minimum number of bits for the color buffer, excluding alpha. `None` means "don't care". The default is `None`.
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

impl Default for PixelFormatRequirements {
    fn default() -> Self {
        PixelFormatRequirements {
            hardware_accelerated: Some(true),
            color_bits: Some(24),
            float_color_buffer: false,
            alpha_bits: Some(8),
            depth_bits: Some(24),
            stencil_bits: Some(8),
            double_buffer: Some(true),
            multisampling: None,
            stereoscopy: false,
        }
    }
}
