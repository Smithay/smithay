//! Type safe native types for safe egl initialisation

#[cfg(feature = "use_system_lib")]
use crate::backend::egl::EGLGraphicsBackend;
use crate::backend::egl::{
    ffi, get_proc_address, native, wrap_egl_call, BufferAccessError, EGLContext, EGLError, EGLImages,
    EGLSurface, Error, Format, SurfaceCreationError,
};
use std::sync::Arc;

use std::ptr;

use nix::libc::{c_int, c_void};

#[cfg(feature = "wayland_frontend")]
use wayland_server::{protocol::wl_buffer::WlBuffer, Display};
#[cfg(feature = "use_system_lib")]
use wayland_sys::server::wl_display;

use crate::backend::egl::context::{GlAttributes, PixelFormatRequirements};
#[cfg(feature = "renderer_gl")]
use crate::backend::graphics::gl::ffi as gl_ffi;
use crate::backend::graphics::PixelFormat;
use std::cell::{Ref, RefCell, RefMut};
use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::mem::MaybeUninit;

use std::ops::Deref;

/// Wrapper around [`ffi::EGLDisplay`](ffi::egl::types::EGLDisplay) to ensure display is only destroyed
/// once all resources bound to it have been dropped.
pub(crate) struct EGLDisplayHandle {
    handle: ffi::egl::types::EGLDisplay,
}

impl Deref for EGLDisplayHandle {
    type Target = ffi::egl::types::EGLDisplay;

    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

impl Drop for EGLDisplayHandle {
    fn drop(&mut self) {
        unsafe {
            // ignore errors on drop
            ffi::egl::Terminate(self.handle);
        }
    }
}

/// [`EGLDisplay`] represents an initialised EGL environment
pub struct EGLDisplay<B: native::Backend, N: native::NativeDisplay<B>> {
    native: RefCell<N>,
    pub(crate) display: Arc<EGLDisplayHandle>,
    pub(crate) egl_version: (i32, i32),
    pub(crate) extensions: Vec<String>,
    logger: slog::Logger,
    _backend: PhantomData<B>,
}

impl<B: native::Backend, N: native::NativeDisplay<B>> EGLDisplay<B, N> {
    /// Create a new [`EGLDisplay`] from a given [`NativeDisplay`](native::NativeDisplay)
    pub fn new<L>(native: N, logger: L) -> Result<EGLDisplay<B, N>, Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger.into()).new(o!("smithay_module" => "renderer_egl"));
        let ptr = native.ptr()?;

        ffi::egl::LOAD.call_once(|| unsafe {
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
            let proc_address = constrain(|sym| get_proc_address(sym));
            ffi::egl::load_with(&proc_address);
            ffi::egl::BindWaylandDisplayWL::load_with(&proc_address);
            ffi::egl::UnbindWaylandDisplayWL::load_with(&proc_address);
            ffi::egl::QueryWaylandBufferWL::load_with(&proc_address);
        });

        // the first step is to query the list of extensions without any display, if supported
        let dp_extensions = unsafe {
            let p =
                wrap_egl_call(|| ffi::egl::QueryString(ffi::egl::NO_DISPLAY, ffi::egl::EXTENSIONS as i32))
                    .map_err(Error::InitFailed)?;

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

        let display = unsafe {
            B::get_display(ptr, |e: &str| dp_extensions.iter().any(|s| s == e), log.clone())
                .map_err(Error::DisplayNotSupported)?
        };

        let egl_version = {
            let mut major: MaybeUninit<ffi::egl::types::EGLint> = MaybeUninit::uninit();
            let mut minor: MaybeUninit<ffi::egl::types::EGLint> = MaybeUninit::uninit();

            wrap_egl_call(|| unsafe {
                ffi::egl::Initialize(display, major.as_mut_ptr(), minor.as_mut_ptr())
            })
            .map_err(Error::InitFailed)?;

            let major = unsafe { major.assume_init() };
            let minor = unsafe { minor.assume_init() };

            info!(log, "EGL Initialized");
            info!(log, "EGL Version: {:?}", (major, minor));

            (major, minor)
        };

        // the list of extensions supported by the client once initialized is different from the
        // list of extensions obtained earlier
        let extensions = if egl_version >= (1, 2) {
            let p = unsafe {
                CStr::from_ptr(
                    wrap_egl_call(|| ffi::egl::QueryString(display, ffi::egl::EXTENSIONS as i32))
                        .map_err(Error::InitFailed)?,
                )
            };
            let list = String::from_utf8(p.to_bytes().to_vec()).unwrap_or_else(|_| String::new());
            list.split(' ').map(|e| e.to_string()).collect::<Vec<_>>()
        } else {
            vec![]
        };
        info!(log, "EGL Extensions: {:?}", extensions);

        if egl_version <= (1, 2) {
            return Err(Error::OpenGlesNotSupported(None));
        }
        wrap_egl_call(|| unsafe { ffi::egl::BindAPI(ffi::egl::OPENGL_ES_API) })
            .map_err(|source| Error::OpenGlesNotSupported(Some(source)))?;

        Ok(EGLDisplay {
            native: RefCell::new(native),
            display: Arc::new(EGLDisplayHandle { handle: display }),
            egl_version,
            extensions,
            logger: log,
            _backend: PhantomData,
        })
    }

    /// Finds a compatible [`EGLConfig`] for a given set of requirements
    pub fn choose_config(
        &self,
        attributes: GlAttributes,
        reqs: PixelFormatRequirements,
    ) -> Result<(PixelFormat, ffi::egl::types::EGLConfig), Error> {
        let descriptor = {
            let mut out: Vec<c_int> = Vec::with_capacity(37);

            if self.egl_version >= (1, 2) {
                trace!(self.logger, "Setting COLOR_BUFFER_TYPE to RGB_BUFFER");
                out.push(ffi::egl::COLOR_BUFFER_TYPE as c_int);
                out.push(ffi::egl::RGB_BUFFER as c_int);
            }

            trace!(self.logger, "Setting SURFACE_TYPE to WINDOW");

            out.push(ffi::egl::SURFACE_TYPE as c_int);
            // TODO: Some versions of Mesa report a BAD_ATTRIBUTE error
            // if we ask for PBUFFER_BIT as well as WINDOW_BIT
            out.push((ffi::egl::WINDOW_BIT) as c_int);

            match attributes.version {
                Some((3, _)) => {
                    if self.egl_version < (1, 3) {
                        error!(
                            self.logger,
                            "OpenglES 3.* is not supported on EGL Versions lower then 1.3"
                        );
                        return Err(Error::NoAvailablePixelFormat);
                    }
                    trace!(self.logger, "Setting RENDERABLE_TYPE to OPENGL_ES3");
                    out.push(ffi::egl::RENDERABLE_TYPE as c_int);
                    out.push(ffi::egl::OPENGL_ES3_BIT as c_int);
                    trace!(self.logger, "Setting CONFORMANT to OPENGL_ES3");
                    out.push(ffi::egl::CONFORMANT as c_int);
                    out.push(ffi::egl::OPENGL_ES3_BIT as c_int);
                }
                Some((2, _)) => {
                    if self.egl_version < (1, 3) {
                        error!(
                            self.logger,
                            "OpenglES 2.* is not supported on EGL Versions lower then 1.3"
                        );
                        return Err(Error::NoAvailablePixelFormat);
                    }
                    trace!(self.logger, "Setting RENDERABLE_TYPE to OPENGL_ES2");
                    out.push(ffi::egl::RENDERABLE_TYPE as c_int);
                    out.push(ffi::egl::OPENGL_ES2_BIT as c_int);
                    trace!(self.logger, "Setting CONFORMANT to OPENGL_ES2");
                    out.push(ffi::egl::CONFORMANT as c_int);
                    out.push(ffi::egl::OPENGL_ES2_BIT as c_int);
                }
                Some(ver) => {
                    return Err(Error::OpenGlVersionNotSupported(ver));
                }
                None => {
                    return Err(Error::OpenGlVersionNotSupported((0, 0)));
                }
            };

            reqs.create_attributes(&mut out, &self.logger)
                .map_err(|()| Error::NoAvailablePixelFormat)?;

            out.push(ffi::egl::NONE as c_int);
            out
        };

        // calling `eglChooseConfig`
        let mut num_configs = 0;
        wrap_egl_call(|| unsafe {
            ffi::egl::ChooseConfig(
                **self.display,
                descriptor.as_ptr(),
                std::ptr::null_mut(),
                0,
                &mut num_configs,
            )
        })
        .map_err(Error::ConfigFailed)?;
        if num_configs == 0 {
            return Err(Error::NoAvailablePixelFormat);
        }

        let mut config_ids: Vec<ffi::egl::types::EGLConfig> = Vec::with_capacity(num_configs as usize);
        wrap_egl_call(|| unsafe {
            ffi::egl::ChooseConfig(
                **self.display,
                descriptor.as_ptr(),
                config_ids.as_mut_ptr(),
                num_configs,
                &mut num_configs,
            )
        })
        .map_err(Error::ConfigFailed)?;
        unsafe {
            config_ids.set_len(num_configs as usize);
        }

        // TODO: Deeper swap intervals might have some uses
        let desired_swap_interval = if attributes.vsync { 1 } else { 0 };

        let config_ids = config_ids
            .into_iter()
            .map(|config| unsafe {
                let mut min_swap_interval = 0;
                wrap_egl_call(|| {
                    ffi::egl::GetConfigAttrib(
                        **self.display,
                        config,
                        ffi::egl::MIN_SWAP_INTERVAL as ffi::egl::types::EGLint,
                        &mut min_swap_interval,
                    )
                })?;

                if desired_swap_interval < min_swap_interval {
                    return Ok(None);
                }

                let mut max_swap_interval = 0;
                wrap_egl_call(|| {
                    ffi::egl::GetConfigAttrib(
                        **self.display,
                        config,
                        ffi::egl::MAX_SWAP_INTERVAL as ffi::egl::types::EGLint,
                        &mut max_swap_interval,
                    )
                })?;

                if desired_swap_interval > max_swap_interval {
                    return Ok(None);
                }

                Ok(Some(config))
            })
            .collect::<Result<Vec<Option<ffi::egl::types::EGLConfig>>, EGLError>>()
            .map_err(Error::ConfigFailed)?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        if config_ids.is_empty() {
            return Err(Error::NoAvailablePixelFormat);
        }

        // TODO: Improve config selection
        let config_id = config_ids[0];

        // analyzing each config
        macro_rules! attrib {
            ($display:expr, $config:expr, $attr:expr) => {{
                let mut value = MaybeUninit::uninit();
                wrap_egl_call(|| {
                    ffi::egl::GetConfigAttrib(
                        **$display,
                        $config,
                        $attr as ffi::egl::types::EGLint,
                        value.as_mut_ptr(),
                    )
                })
                .map_err(Error::ConfigFailed)?;
                value.assume_init()
            }};
        };

        let desc = unsafe {
            PixelFormat {
                hardware_accelerated: attrib!(self.display, config_id, ffi::egl::CONFIG_CAVEAT)
                    != ffi::egl::SLOW_CONFIG as i32,
                color_bits: attrib!(self.display, config_id, ffi::egl::RED_SIZE) as u8
                    + attrib!(self.display, config_id, ffi::egl::BLUE_SIZE) as u8
                    + attrib!(self.display, config_id, ffi::egl::GREEN_SIZE) as u8,
                alpha_bits: attrib!(self.display, config_id, ffi::egl::ALPHA_SIZE) as u8,
                depth_bits: attrib!(self.display, config_id, ffi::egl::DEPTH_SIZE) as u8,
                stencil_bits: attrib!(self.display, config_id, ffi::egl::STENCIL_SIZE) as u8,
                stereoscopy: false,
                multisampling: match attrib!(self.display, config_id, ffi::egl::SAMPLES) {
                    0 | 1 => None,
                    a => Some(a as u16),
                },
                srgb: false, // TODO: use EGL_KHR_gl_colorspace to know that
            }
        };

        info!(self.logger, "Selected color format: {:?}", desc);

        Ok((desc, config_id))
    }

    /// Create a new [`EGLContext`](::backend::egl::EGLContext)
    pub fn create_context(
        &self,
        attributes: GlAttributes,
        reqs: PixelFormatRequirements,
    ) -> Result<EGLContext, Error> {
        EGLContext::new(&self, attributes, reqs, self.logger.clone())
    }

    /// Creates a surface for rendering
    pub fn create_surface(
        &self,
        pixel_format: PixelFormat,
        double_buffer: Option<bool>,
        config: ffi::egl::types::EGLConfig,
        args: N::Arguments,
    ) -> Result<EGLSurface<B::Surface>, SurfaceCreationError<N::Error>> {
        trace!(self.logger, "Creating EGL window surface.");
        let surface = self
            .native
            .borrow_mut()
            .create_surface(args)
            .map_err(SurfaceCreationError::NativeSurfaceCreationFailed)?;

        EGLSurface::new(
            self.display.clone(),
            pixel_format,
            double_buffer,
            config,
            surface,
            self.logger.clone(),
        )
        .map(|x| {
            debug!(self.logger, "EGL surface successfully created");
            x
        })
        .map_err(SurfaceCreationError::EGLSurfaceCreationFailed)
    }

    /// Returns the runtime egl version of this display
    pub fn get_egl_version(&self) -> (i32, i32) {
        self.egl_version
    }

    /// Returns the supported extensions of this display
    pub fn get_extensions(&self) -> Vec<String> {
        self.extensions.clone()
    }

    /// Borrow the underlying native display.
    ///
    /// This follows the same semantics as [`std::cell:RefCell`](std::cell::RefCell).
    /// Multiple read-only borrows are possible. Borrowing the
    /// backend while there is a mutable reference will panic.
    pub fn borrow(&self) -> Ref<'_, N> {
        self.native.borrow()
    }

    /// Borrow the underlying native display mutably.
    ///
    /// This follows the same semantics as [`std::cell:RefCell`](std::cell::RefCell).
    /// Holding any other borrow while trying to borrow the backend
    /// mutably will panic. Note that EGL will borrow the display
    /// mutably during surface creation.
    pub fn borrow_mut(&self) -> RefMut<'_, N> {
        self.native.borrow_mut()
    }
}

#[cfg(feature = "use_system_lib")]
impl<B: native::Backend, N: native::NativeDisplay<B>> EGLGraphicsBackend for EGLDisplay<B, N> {
    /// Binds this EGL display to the given Wayland display.
    ///
    /// This will allow clients to utilize EGL to create hardware-accelerated
    /// surfaces. The server will need to be able to handle EGL-[`WlBuffer`]s.
    ///
    /// ## Errors
    ///
    /// This might return [`EglExtensionNotSupported`](ErrorKind::EglExtensionNotSupported)
    /// if binding is not supported by the EGL implementation.
    ///
    /// This might return [`OtherEGLDisplayAlreadyBound`](ErrorKind::OtherEGLDisplayAlreadyBound)
    /// if called for the same [`Display`] multiple times, as only one egl display may be bound at any given time.
    fn bind_wl_display(&self, display: &Display) -> Result<EGLBufferReader, Error> {
        if !self.extensions.iter().any(|s| s == "EGL_WL_bind_wayland_display") {
            return Err(Error::EglExtensionNotSupported(&["EGL_WL_bind_wayland_display"]));
        }
        wrap_egl_call(|| unsafe {
            ffi::egl::BindWaylandDisplayWL(**self.display, display.c_ptr() as *mut _)
        })
        .map_err(Error::OtherEGLDisplayAlreadyBound)?;
        Ok(EGLBufferReader::new(self.display.clone(), display.c_ptr()))
    }
}

/// Type to receive [`EGLImages`] for EGL-based [`WlBuffer`]s.
///
/// Can be created by using [`EGLGraphicsBackend::bind_wl_display`].
#[cfg(feature = "use_system_lib")]
pub struct EGLBufferReader {
    display: Arc<EGLDisplayHandle>,
    wayland: *mut wl_display,
    #[cfg(feature = "renderer_gl")]
    gl: gl_ffi::Gles2,
}

#[cfg(feature = "use_system_lib")]
impl EGLBufferReader {
    fn new(display: Arc<EGLDisplayHandle>, wayland: *mut wl_display) -> Self {
        #[cfg(feature = "renderer_gl")]
        let gl = gl_ffi::Gles2::load_with(|s| get_proc_address(s) as *const _);

        Self {
            display,
            wayland,
            #[cfg(feature = "renderer_gl")]
            gl,
        }
    }

    /// Try to receive [`EGLImages`] from a given [`WlBuffer`].
    ///
    /// In case the buffer is not managed by EGL (but e.g. the [`wayland::shm` module](::wayland::shm))
    /// a [`BufferAccessError::NotManaged`](::backend::egl::BufferAccessError::NotManaged) is returned with the original buffer
    /// to render it another way.
    pub fn egl_buffer_contents(
        &self,
        buffer: WlBuffer,
    ) -> ::std::result::Result<EGLImages, BufferAccessError> {
        let mut format: i32 = 0;
        if wrap_egl_call(|| unsafe {
            ffi::egl::QueryWaylandBufferWL(
                **self.display,
                buffer.as_ref().c_ptr() as _,
                ffi::egl::EGL_TEXTURE_FORMAT,
                &mut format,
            )
        }).map_err(|source| BufferAccessError::NotManaged(buffer.clone(), source))? == ffi::egl::FALSE {
            return Err(BufferAccessError::NotManaged(buffer.clone(), EGLError::BadParameter));
        }
        
        let format = match format {
            x if x == ffi::egl::TEXTURE_RGB as i32 => Format::RGB,
            x if x == ffi::egl::TEXTURE_RGBA as i32 => Format::RGBA,
            ffi::egl::TEXTURE_EXTERNAL_WL => Format::External,
            ffi::egl::TEXTURE_Y_UV_WL => Format::Y_UV,
            ffi::egl::TEXTURE_Y_U_V_WL => Format::Y_U_V,
            ffi::egl::TEXTURE_Y_XUXV_WL => Format::Y_XUXV,
            x => panic!("EGL returned invalid texture type: {}", x),
        };

        let mut width: i32 = 0;
        wrap_egl_call(|| unsafe {
            ffi::egl::QueryWaylandBufferWL(
                **self.display,
                buffer.as_ref().c_ptr() as _,
                ffi::egl::WIDTH as i32,
                &mut width,
            )
        })
        .map_err(|source| BufferAccessError::NotManaged(buffer.clone(), source))?;

        let mut height: i32 = 0;
        wrap_egl_call(|| unsafe {
            ffi::egl::QueryWaylandBufferWL(
                **self.display,
                buffer.as_ref().c_ptr() as _,
                ffi::egl::HEIGHT as i32,
                &mut height,
            )
        })
        .map_err(|source| BufferAccessError::NotManaged(buffer.clone(), source))?;

        let mut inverted: i32 = 0;
        wrap_egl_call(|| unsafe {
            ffi::egl::QueryWaylandBufferWL(
                **self.display,
                buffer.as_ref().c_ptr() as _,
                ffi::egl::WAYLAND_Y_INVERTED_WL,
                &mut inverted,
            )
        })
        .map_err(|source| BufferAccessError::NotManaged(buffer.clone(), source))?;

        let mut images = Vec::with_capacity(format.num_planes());
        for i in 0..format.num_planes() {
            let mut out = Vec::with_capacity(3);
            out.push(ffi::egl::WAYLAND_PLANE_WL as i32);
            out.push(i as i32);
            out.push(ffi::egl::NONE as i32);

            images.push({
                wrap_egl_call(|| unsafe {
                    ffi::egl::CreateImageKHR(
                        **self.display,
                        ffi::egl::NO_CONTEXT,
                        ffi::egl::WAYLAND_BUFFER_WL,
                        buffer.as_ref().c_ptr() as *mut _,
                        out.as_ptr(),
                    )
                })
                .map_err(BufferAccessError::EGLImageCreationFailed)?
            });
        }

        Ok(EGLImages {
            display: self.display.clone(),
            width: width as u32,
            height: height as u32,
            y_inverted: inverted != 0,
            format,
            images,
            buffer,
            #[cfg(feature = "renderer_gl")]
            gl: self.gl.clone(),
        })
    }

    /// Try to receive the dimensions of a given [`WlBuffer`].
    ///
    /// In case the buffer is not managed by EGL (but e.g. the [`wayland::shm` module](::wayland::shm)) or the
    /// context has been lost, `None` is returned.
    pub fn egl_buffer_dimensions(&self, buffer: &WlBuffer) -> Option<(i32, i32)> {
        let mut width: i32 = 0;
        if unsafe {
            ffi::egl::QueryWaylandBufferWL(
                **self.display,
                buffer.as_ref().c_ptr() as _,
                ffi::egl::WIDTH as _,
                &mut width,
            ) == 0
        } {
            return None;
        }

        let mut height: i32 = 0;
        if unsafe {
            ffi::egl::QueryWaylandBufferWL(
                **self.display,
                buffer.as_ref().c_ptr() as _,
                ffi::egl::HEIGHT as _,
                &mut height,
            ) == 0
        } {
            return None;
        }

        Some((width, height))
    }
}

#[cfg(feature = "use_system_lib")]
impl Drop for EGLBufferReader {
    fn drop(&mut self) {
        if !self.wayland.is_null() {
            unsafe {
                // ignore errors on drop
                ffi::egl::UnbindWaylandDisplayWL(**self.display, self.wayland as _);
            }
        }
    }
}
