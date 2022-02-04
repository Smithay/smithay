//! Type safe native types for safe egl initialisation

use std::collections::HashSet;
use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::sync::Arc;
#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use std::sync::{Mutex, Weak};

use libc::c_void;
use nix::libc::c_int;
#[cfg(all(feature = "use_system_lib", feature = "wayland_frontend"))]
use wayland_server::{protocol::wl_buffer::WlBuffer, Display};
#[cfg(feature = "use_system_lib")]
use wayland_sys::server::wl_display;

use crate::backend::allocator::{dmabuf::Dmabuf, Buffer, Format as DrmFormat, Fourcc, Modifier};
use crate::backend::egl::{
    context::{GlAttributes, PixelFormatRequirements},
    ffi,
    ffi::egl::types::EGLImage,
    native::EGLNativeDisplay,
    wrap_egl_call, EGLError, Error,
};
#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use crate::backend::egl::{BufferAccessError, EGLBuffer, Format};

use slog::{debug, error, info, o, trace, warn};

#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
lazy_static::lazy_static! {
    pub(crate) static ref BUFFER_READER: Mutex<Option<WeakBufferReader>> = Mutex::new(None);
}

/// Wrapper around [`ffi::EGLDisplay`](ffi::egl::types::EGLDisplay) to ensure display is only destroyed
/// once all resources bound to it have been dropped.
#[derive(Debug)]
pub struct EGLDisplayHandle {
    /// ffi EGLDisplay ptr
    pub handle: ffi::egl::types::EGLDisplay,
}
// EGLDisplay has an internal Mutex
unsafe impl Send for EGLDisplayHandle {}
unsafe impl Sync for EGLDisplayHandle {}

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
#[derive(Debug, Clone)]
pub struct EGLDisplay {
    pub(crate) display: Arc<EGLDisplayHandle>,
    pub(crate) egl_version: (i32, i32),
    pub(crate) extensions: Vec<String>,
    pub(crate) dmabuf_import_formats: HashSet<DrmFormat>,
    pub(crate) dmabuf_render_formats: HashSet<DrmFormat>,
    surface_type: ffi::EGLint,
    logger: slog::Logger,
}

fn select_platform_display<N: EGLNativeDisplay + 'static>(
    native: &N,
    dp_extensions: &[String],
    log: &::slog::Logger,
) -> Result<*const c_void, Error> {
    for platform in native.supported_platforms() {
        debug!(log, "Trying EGL platform: {}", platform.platform_name);

        let log = log.new(o!("platform" => format!("{:?}", platform)));

        let missing_extensions = platform
            .required_extensions
            .iter()
            .filter(|ext| !dp_extensions.iter().any(|x| x == *ext))
            .collect::<Vec<_>>();

        if !missing_extensions.is_empty() {
            info!(
                log,
                "Skipping EGL platform because one or more required extensions are not supported. Missing extensions: {:?}", missing_extensions
            );
            continue;
        }

        let display = unsafe {
            wrap_egl_call(|| {
                ffi::egl::GetPlatformDisplayEXT(
                    platform.platform,
                    platform.native_display,
                    platform.attrib_list.as_ptr(),
                )
            })
            .map_err(Error::DisplayCreationError)
        };

        let display = match display {
            Ok(display) => {
                if display == ffi::egl::NO_DISPLAY {
                    info!(log, "Skipping platform because the display is not supported");
                    continue;
                }

                display
            }
            Err(err) => {
                info!(
                    log,
                    "Skipping platform because of an display creation error: {:?}", err
                );
                continue;
            }
        };

        info!(
            log,
            "Successfully selected EGL platform: {}", platform.platform_name
        );
        return Ok(display);
    }

    error!(log, "Unable to find suitable EGL platform");
    Err(Error::DisplayNotSupported)
}

impl EGLDisplay {
    /// Create a new [`EGLDisplay`] from a given [`EGLNativeDisplay`]
    pub fn new<N, L>(native: &N, logger: L) -> Result<EGLDisplay, Error>
    where
        N: EGLNativeDisplay + 'static,
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger.into()).new(o!("smithay_module" => "backend_egl"));

        let dp_extensions = ffi::make_sure_egl_is_loaded()?;
        debug!(log, "Supported EGL client extensions: {:?}", dp_extensions);
        // we create an EGLDisplay
        let display = select_platform_display(native, &dp_extensions, &log)?;

        // We can then query the egl api version
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
        info!(log, "Supported EGL display extensions: {:?}", extensions);

        let (dmabuf_import_formats, dmabuf_render_formats) =
            get_dmabuf_formats(&display, &extensions, &log).map_err(Error::DisplayCreationError)?;

        // egl <= 1.2 does not support OpenGL ES (maybe we want to support OpenGL in the future?)
        if egl_version <= (1, 2) {
            return Err(Error::OpenGlesNotSupported(None));
        }
        wrap_egl_call(|| unsafe { ffi::egl::BindAPI(ffi::egl::OPENGL_ES_API) })
            .map_err(|source| Error::OpenGlesNotSupported(Some(source)))?;

        Ok(EGLDisplay {
            display: Arc::new(EGLDisplayHandle { handle: display }),
            surface_type: native.surface_type(),
            egl_version,
            extensions,
            dmabuf_import_formats,
            dmabuf_render_formats,
            logger: log,
        })
    }

    /// Finds a compatible EGLConfig for a given set of requirements
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

            trace!(self.logger, "Setting SURFACE_TYPE to {}", self.surface_type);

            out.push(ffi::egl::SURFACE_TYPE as c_int);
            out.push(self.surface_type);

            match attributes.version {
                (3, _) => {
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
                (2, _) => {
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
                ver => {
                    return Err(Error::OpenGlVersionNotSupported(ver));
                }
            };

            reqs.create_attributes(&mut out, &self.logger);
            out.push(ffi::egl::NONE as c_int);
            out
        };

        // Try to find configs that match out criteria
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

        if config_ids.is_empty() {
            return Err(Error::NoAvailablePixelFormat);
        }

        let desired_swap_interval = if attributes.vsync { 1 } else { 0 };
        // try to select a config with the desired_swap_interval
        // (but don't fail, as the margin might be very small on some cards and most configs are fine)
        let config_id = config_ids
            .iter()
            .copied()
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
            .next()
            .unwrap_or_else(|| config_ids[0]);

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
        }

        // return the format that was selected for our config
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

    /// Get a handle to the underlying raw EGLDisplay handle
    pub fn get_display_handle(&self) -> Arc<EGLDisplayHandle> {
        self.display.clone()
    }

    /// Returns the runtime egl version of this display
    pub fn get_egl_version(&self) -> (i32, i32) {
        self.egl_version
    }

    /// Returns the supported extensions of this display
    pub fn get_extensions(&self) -> Vec<String> {
        self.extensions.clone()
    }

    /// Imports a [`Dmabuf`] as an [`EGLImage`]
    pub fn create_image_from_dmabuf(&self, dmabuf: &Dmabuf) -> Result<EGLImage, Error> {
        if !self.extensions.iter().any(|s| s == "EGL_KHR_image_base")
            && !self
                .extensions
                .iter()
                .any(|s| s == "EGL_EXT_image_dma_buf_import")
        {
            return Err(Error::EglExtensionNotSupported(&[
                "EGL_KHR_image_base",
                "EGL_EXT_image_dma_buf_import",
            ]));
        }

        if dmabuf.has_modifier()
            && !self
                .extensions
                .iter()
                .any(|s| s == "EGL_EXT_image_dma_buf_import_modifiers")
        {
            return Err(Error::EglExtensionNotSupported(&[
                "EGL_EXT_image_dma_buf_import_modifiers",
            ]));
        };

        let mut out: Vec<c_int> = Vec::with_capacity(50);

        out.extend(&[
            ffi::egl::WIDTH as i32,
            dmabuf.width() as i32,
            ffi::egl::HEIGHT as i32,
            dmabuf.height() as i32,
            ffi::egl::LINUX_DRM_FOURCC_EXT as i32,
            dmabuf.format().code as u32 as i32,
        ]);

        let names = [
            [
                ffi::egl::DMA_BUF_PLANE0_FD_EXT,
                ffi::egl::DMA_BUF_PLANE0_OFFSET_EXT,
                ffi::egl::DMA_BUF_PLANE0_PITCH_EXT,
                ffi::egl::DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                ffi::egl::DMA_BUF_PLANE0_MODIFIER_HI_EXT,
            ],
            [
                ffi::egl::DMA_BUF_PLANE1_FD_EXT,
                ffi::egl::DMA_BUF_PLANE1_OFFSET_EXT,
                ffi::egl::DMA_BUF_PLANE1_PITCH_EXT,
                ffi::egl::DMA_BUF_PLANE1_MODIFIER_LO_EXT,
                ffi::egl::DMA_BUF_PLANE1_MODIFIER_HI_EXT,
            ],
            [
                ffi::egl::DMA_BUF_PLANE2_FD_EXT,
                ffi::egl::DMA_BUF_PLANE2_OFFSET_EXT,
                ffi::egl::DMA_BUF_PLANE2_PITCH_EXT,
                ffi::egl::DMA_BUF_PLANE2_MODIFIER_LO_EXT,
                ffi::egl::DMA_BUF_PLANE2_MODIFIER_HI_EXT,
            ],
            [
                ffi::egl::DMA_BUF_PLANE3_FD_EXT,
                ffi::egl::DMA_BUF_PLANE3_OFFSET_EXT,
                ffi::egl::DMA_BUF_PLANE3_PITCH_EXT,
                ffi::egl::DMA_BUF_PLANE3_MODIFIER_LO_EXT,
                ffi::egl::DMA_BUF_PLANE3_MODIFIER_HI_EXT,
            ],
        ];

        for (i, ((fd, offset), stride)) in dmabuf
            .handles()
            .zip(dmabuf.offsets())
            .zip(dmabuf.strides())
            .enumerate()
        {
            out.extend(&[
                names[i][0] as i32,
                fd,
                names[i][1] as i32,
                offset as i32,
                names[i][2] as i32,
                stride as i32,
            ]);
            if dmabuf.has_modifier() {
                out.extend(&[
                    names[i][3] as i32,
                    (Into::<u64>::into(dmabuf.format().modifier) & 0xFFFFFFFF) as i32,
                    names[i][4] as i32,
                    (Into::<u64>::into(dmabuf.format().modifier) >> 32) as i32,
                ])
            }
        }

        out.push(ffi::egl::NONE as i32);

        unsafe {
            let image = ffi::egl::CreateImageKHR(
                **self.display,
                ffi::egl::NO_CONTEXT,
                ffi::egl::LINUX_DMA_BUF_EXT,
                std::ptr::null(),
                out.as_ptr(),
            );

            if image == ffi::egl::NO_IMAGE_KHR {
                Err(Error::EGLImageCreationFailed)
            } else {
                Ok(image)
            }
        }
    }

    /// Binds this EGL display to the given Wayland display.
    ///
    /// This will allow clients to utilize EGL to create hardware-accelerated
    /// surfaces. The server will need to be able to handle EGL-[`WlBuffer`]s.
    ///
    /// ## Errors
    ///
    /// This might return [`EglExtensionNotSupported`](Error::EglExtensionNotSupported)
    /// if binding is not supported by the EGL implementation.
    ///
    /// This might return [`OtherEGLDisplayAlreadyBound`](Error::OtherEGLDisplayAlreadyBound)
    /// if called for the same [`Display`] multiple times, as only one egl display may be bound at any given time.
    #[cfg(all(feature = "use_system_lib", feature = "wayland_frontend"))]
    pub fn bind_wl_display<D: 'static>(&self, display: &Display<D>) -> Result<EGLBufferReader, Error> {
        let display_ptr = display.backend().lock().unwrap().display_ptr();
        if !self.extensions.iter().any(|s| s == "EGL_WL_bind_wayland_display") {
            return Err(Error::EglExtensionNotSupported(&["EGL_WL_bind_wayland_display"]));
        }
        wrap_egl_call(|| unsafe { ffi::egl::BindWaylandDisplayWL(**self.display, display_ptr as *mut _) })
            .map_err(Error::OtherEGLDisplayAlreadyBound)?;
        let reader = EGLBufferReader::new(self.display.clone(), display_ptr, self.logger.clone());
        let mut global = BUFFER_READER.lock().unwrap();
        if global.as_ref().and_then(|x| x.upgrade()).is_some() {
            warn!(
                self.logger,
                "Double bind_wl_display, smithay does not support this, please report"
            );
        }
        *global = Some(WeakBufferReader {
            display: Arc::downgrade(&self.display),
            logger: self.logger.clone(),
        });
        Ok(reader)
    }
}

fn get_dmabuf_formats(
    display: &ffi::egl::types::EGLDisplay,
    extensions: &[String],
    log: &::slog::Logger,
) -> Result<(HashSet<DrmFormat>, HashSet<DrmFormat>), EGLError> {
    if !extensions.iter().any(|s| s == "EGL_EXT_image_dma_buf_import") {
        warn!(log, "Dmabuf import extension not available");
        return Ok((HashSet::new(), HashSet::new()));
    }

    let formats = {
        // when we only have the image_dmabuf_import extension we can't query
        // which formats are supported. These two are on almost always
        // supported; it's the intended way to just try to create buffers.
        // Just a guess but better than not supporting dmabufs at all,
        // given that the modifiers extension isn't supported everywhere.
        if !extensions
            .iter()
            .any(|s| s == "EGL_EXT_image_dma_buf_import_modifiers")
        {
            vec![Fourcc::Argb8888, Fourcc::Xrgb8888]
        } else {
            let mut num = 0i32;
            wrap_egl_call(|| unsafe {
                ffi::egl::QueryDmaBufFormatsEXT(*display, 0, std::ptr::null_mut(), &mut num as *mut _)
            })?;
            if num == 0 {
                return Ok((HashSet::new(), HashSet::new()));
            }
            let mut formats: Vec<u32> = Vec::with_capacity(num as usize);
            wrap_egl_call(|| unsafe {
                ffi::egl::QueryDmaBufFormatsEXT(
                    *display,
                    num,
                    formats.as_mut_ptr() as *mut _,
                    &mut num as *mut _,
                )
            })?;
            unsafe {
                formats.set_len(num as usize);
            }
            formats
                .into_iter()
                .flat_map(|x| Fourcc::try_from(x).ok())
                .collect::<Vec<_>>()
        }
    };

    let mut texture_formats = HashSet::new();
    let mut render_formats = HashSet::new();

    for fourcc in formats {
        let mut num = 0i32;
        wrap_egl_call(|| unsafe {
            ffi::egl::QueryDmaBufModifiersEXT(
                *display,
                fourcc as i32,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut num as *mut _,
            )
        })?;

        if num == 0 {
            texture_formats.insert(DrmFormat {
                code: fourcc,
                modifier: Modifier::Invalid,
            });
            render_formats.insert(DrmFormat {
                code: fourcc,
                modifier: Modifier::Invalid,
            });
        } else {
            let mut mods: Vec<u64> = Vec::with_capacity(num as usize);
            let mut external: Vec<ffi::egl::types::EGLBoolean> = Vec::with_capacity(num as usize);

            wrap_egl_call(|| unsafe {
                ffi::egl::QueryDmaBufModifiersEXT(
                    *display,
                    fourcc as i32,
                    num,
                    mods.as_mut_ptr(),
                    external.as_mut_ptr(),
                    &mut num as *mut _,
                )
            })?;

            unsafe {
                mods.set_len(num as usize);
                external.set_len(num as usize);
            }

            for (modifier, external_only) in mods.into_iter().zip(external.into_iter()) {
                let format = DrmFormat {
                    code: fourcc,
                    modifier: Modifier::from(modifier),
                };
                texture_formats.insert(format);
                if external_only == 0 {
                    render_formats.insert(format);
                }
            }
        }
    }

    trace!(log, "Supported dmabuf import formats: {:?}", texture_formats);
    trace!(log, "Supported dmabuf render formats: {:?}", render_formats);

    Ok((texture_formats, render_formats))
}

/// Type to receive [`EGLBuffer`] for EGL-based [`WlBuffer`]s.
///
/// Can be created by using [`EGLDisplay::bind_wl_display`].
#[cfg(feature = "use_system_lib")]
#[derive(Debug, Clone)]
pub struct EGLBufferReader {
    display: Arc<EGLDisplayHandle>,
    wayland: Option<Arc<*mut wl_display>>,
    logger: ::slog::Logger,
}

#[cfg(feature = "use_system_lib")]
pub(crate) struct WeakBufferReader {
    display: Weak<EGLDisplayHandle>,
    logger: ::slog::Logger,
}

#[cfg(feature = "use_system_lib")]
impl WeakBufferReader {
    pub fn upgrade(&self) -> Option<EGLBufferReader> {
        Some(EGLBufferReader {
            display: self.display.upgrade()?,
            wayland: None,
            logger: self.logger.clone(),
        })
    }
}

#[cfg(feature = "use_system_lib")]
unsafe impl Send for EGLBufferReader {}

#[cfg(feature = "use_system_lib")]
impl EGLBufferReader {
    fn new<L>(display: Arc<EGLDisplayHandle>, wayland: *mut wl_display, log: L) -> Self
    where
        L: Into<Option<::slog::Logger>>,
    {
        let logger = crate::slog_or_fallback(log.into()).new(o!("smithay_module" => "egl_buffer_reader"));

        Self {
            display,
            wayland: Some(Arc::new(wayland)),
            logger,
        }
    }

    /// Try to receive [`EGLBuffer`] from a given [`WlBuffer`].
    ///
    /// In case the buffer is not managed by EGL (but e.g. the [`wayland::shm` module](crate::wayland::shm))
    /// a [`BufferAccessError::NotManaged`](crate::backend::egl::BufferAccessError::NotManaged) is returned.
    pub fn egl_buffer_contents(
        &self,
        dh: &mut wayland_server::DisplayHandle<'_>,
        buffer: &WlBuffer,
    ) -> ::std::result::Result<EGLBuffer, BufferAccessError> {
        use wayland_server::Resource;
        if dh.get_object_data(buffer.id()).is_err() {
            debug!(self.logger, "Suplied buffer is no longer alive");
            return Err(BufferAccessError::NotManaged(EGLError::BadParameter));
        }

        let mut format: i32 = 0;
        let query = wrap_egl_call(|| unsafe {
            ffi::egl::QueryWaylandBufferWL(
                **self.display,
                buffer.id().as_ptr() as _,
                ffi::egl::EGL_TEXTURE_FORMAT,
                &mut format,
            )
        })
        .map_err(BufferAccessError::NotManaged)?;
        if query == ffi::egl::FALSE {
            return Err(BufferAccessError::NotManaged(EGLError::BadParameter));
        }

        let format = match format {
            x if x == ffi::egl::TEXTURE_RGB as i32 => Format::RGB,
            x if x == ffi::egl::TEXTURE_RGBA as i32 => Format::RGBA,
            ffi::egl::TEXTURE_EXTERNAL_WL => Format::External,
            ffi::egl::TEXTURE_Y_UV_WL => {
                return Err(BufferAccessError::UnsupportedMultiPlanarFormat(Format::Y_UV))
            }
            ffi::egl::TEXTURE_Y_U_V_WL => {
                return Err(BufferAccessError::UnsupportedMultiPlanarFormat(Format::Y_U_V))
            }
            ffi::egl::TEXTURE_Y_XUXV_WL => {
                return Err(BufferAccessError::UnsupportedMultiPlanarFormat(Format::Y_XUXV))
            }
            x => panic!("EGL returned invalid texture type: {}", x),
        };

        let mut width: i32 = 0;
        wrap_egl_call(|| unsafe {
            ffi::egl::QueryWaylandBufferWL(
                **self.display,
                buffer.id().as_ptr() as _,
                ffi::egl::WIDTH as i32,
                &mut width,
            )
        })
        .map_err(BufferAccessError::NotManaged)?;

        let mut height: i32 = 0;
        wrap_egl_call(|| unsafe {
            ffi::egl::QueryWaylandBufferWL(
                **self.display,
                buffer.id().as_ptr() as _,
                ffi::egl::HEIGHT as i32,
                &mut height,
            )
        })
        .map_err(BufferAccessError::NotManaged)?;

        let y_inverted = {
            let mut inverted: i32 = 0;

            // Query the egl buffer with EGL_WAYLAND_Y_INVERTED_WL to retrieve the
            // buffer orientation.
            // The call can either fail, succeed with EGL_TRUE or succeed with EGL_FALSE.
            // The specification for eglQuery defines that unsupported attributes shall return
            // EGL_FALSE. In case of EGL_WAYLAND_Y_INVERTED_WL the specification defines that
            // if EGL_FALSE is returned the value of inverted should be assumed as EGL_TRUE.
            //
            // see: https://www.khronos.org/registry/EGL/extensions/WL/EGL_WL_bind_wayland_display.txt
            match wrap_egl_call(|| unsafe {
                ffi::egl::QueryWaylandBufferWL(
                    **self.display,
                    buffer.id().as_ptr() as _,
                    ffi::egl::WAYLAND_Y_INVERTED_WL,
                    &mut inverted,
                )
            })
            .map_err(BufferAccessError::NotManaged)?
            {
                ffi::egl::TRUE => inverted != 0,
                ffi::egl::FALSE => true,
                _ => unreachable!(),
            }
        };

        let mut images = Vec::with_capacity(format.num_planes());
        for i in 0..format.num_planes() {
            let out = [ffi::egl::WAYLAND_PLANE_WL as i32, i as i32, ffi::egl::NONE as i32];

            images.push({
                wrap_egl_call(|| unsafe {
                    ffi::egl::CreateImageKHR(
                        **self.display,
                        ffi::egl::NO_CONTEXT,
                        ffi::egl::WAYLAND_BUFFER_WL,
                        buffer.id().as_ptr() as *mut _,
                        out.as_ptr(),
                    )
                })
                .map_err(BufferAccessError::EGLImageCreationFailed)?
            });
        }

        Ok(EGLBuffer {
            display: self.display.clone(),
            size: (width, height).into(),
            // y_inverted is negated here because the gles2 renderer
            // already inverts the buffer during rendering.
            y_inverted: !y_inverted,
            format,
            images,
        })
    }

    /// Try to receive the dimensions of a given [`WlBuffer`].
    ///
    /// In case the buffer is not managed by EGL (but e.g. the [`wayland::shm` module](crate::wayland::shm)) or the
    /// context has been lost, `None` is returned.
    pub fn egl_buffer_dimensions(
        &self,
        dh: &mut wayland_server::DisplayHandle<'_>,
        buffer: &WlBuffer,
    ) -> Option<crate::utils::Size<i32, crate::utils::Buffer>> {
        use wayland_server::Resource;
        if dh.get_object_data(buffer.id()).is_err() {
            debug!(self.logger, "Suplied buffer is no longer alive");
            return None;
        }

        let mut width: i32 = 0;
        if unsafe {
            ffi::egl::QueryWaylandBufferWL(
                **self.display,
                buffer.id().as_ptr() as _,
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
                buffer.id().as_ptr() as _,
                ffi::egl::HEIGHT as _,
                &mut height,
            ) == 0
        } {
            return None;
        }

        Some((width, height).into())
    }
}

#[cfg(feature = "use_system_lib")]
impl Drop for EGLBufferReader {
    fn drop(&mut self) {
        if let Some(wayland) = self.wayland.take().and_then(|x| Arc::try_unwrap(x).ok()) {
            if !wayland.is_null() {
                unsafe {
                    // ignore errors on drop
                    ffi::egl::UnbindWaylandDisplayWL(**self.display, wayland as _);
                }
            }
        }
    }
}

/// Describes the pixel format of a framebuffer
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
    /// number of samples used for multisampling if enabled
    pub multisampling: Option<u16>,
    /// is srgb enabled
    pub srgb: bool,
}
