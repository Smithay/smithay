//! EGL context related structs
use std::{
    collections::HashSet,
    os::raw::c_int,
    sync::{atomic::Ordering, Arc},
};

use libc::c_void;

use super::{ffi, wrap_egl_call, Error, MakeCurrentError};
use crate::{
    backend::{
        allocator::Format as DrmFormat,
        egl::{
            display::{EGLDisplay, PixelFormat},
            EGLSurface,
        },
    },
    utils::user_data::UserDataMap,
};

use slog::{info, o, trace};

/// EGL context for rendering
#[derive(Debug)]
pub struct EGLContext {
    context: ffi::egl::types::EGLContext,
    display: EGLDisplay,
    config_id: ffi::egl::types::EGLConfig,
    pixel_format: Option<PixelFormat>,
    user_data: Arc<UserDataMap>,
    externally_managed: bool,
}

// SAFETY: A context can be sent to another thread when this context is not current.
//
// If the context is made current, the safety requirements for calling `EGLDisplay::make_current` must be
// upheld. This means a context which is made current must be unbound (using `unbind`) in order to send this
// context to another thread.
unsafe impl Send for EGLContext {}

impl EGLContext {
    /// Creates a new `EGLContext` from raw handles.
    ///
    /// # Safety
    ///
    /// - The context must be created from the system default EGL library (`dlopen("libEGL.so")`)
    /// - The `display`, `config`, and `context` must be valid for the lifetime of the returned context.
    pub unsafe fn from_raw<L>(
        display: *const c_void,
        config_id: *const c_void,
        context: *const c_void,
        log: L,
    ) -> Result<EGLContext, Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        assert!(!display.is_null(), "EGLDisplay pointer is null");
        assert!(!config_id.is_null(), "EGL configuration id pointer is null");
        assert!(!context.is_null(), "EGLContext pointer is null");
        let log = crate::slog_or_fallback(log.into()).new(o!("smithay_module" => "backend_egl"));

        let display = EGLDisplay::from_raw(display, config_id, log)?;
        let pixel_format = display.get_pixel_format(config_id)?;

        Ok(EGLContext {
            context,
            display,
            config_id,
            pixel_format: Some(pixel_format),
            user_data: Arc::new(UserDataMap::default()),
            externally_managed: true,
        })
    }

    /// Creates a new configless `EGLContext` from a given `EGLDisplay`
    pub fn new<L>(display: &EGLDisplay, log: L) -> Result<EGLContext, Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        Self::new_internal(display, None, None, log)
    }

    /// Create a new [`EGLContext`] from a given `EGLDisplay` and configuration requirements
    pub fn new_with_config<L>(
        display: &EGLDisplay,
        attributes: GlAttributes,
        reqs: PixelFormatRequirements,
        log: L,
    ) -> Result<EGLContext, Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        Self::new_internal(display, None, Some((attributes, reqs)), log)
    }

    /// Create a new configless `EGLContext` from a given `EGLDisplay` sharing resources with another context
    pub fn new_shared<L>(display: &EGLDisplay, share: &EGLContext, log: L) -> Result<EGLContext, Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        Self::new_internal(display, Some(share), None, log)
    }

    /// Create a new `EGLContext` from a given `EGLDisplay` and configuration requirements sharing resources with another context
    pub fn new_shared_with_config<L>(
        display: &EGLDisplay,
        share: &EGLContext,
        attributes: GlAttributes,
        reqs: PixelFormatRequirements,
        log: L,
    ) -> Result<EGLContext, Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        Self::new_internal(display, Some(share), Some((attributes, reqs)), log)
    }

    fn new_internal<L>(
        display: &EGLDisplay,
        shared: Option<&EGLContext>,
        config: Option<(GlAttributes, PixelFormatRequirements)>,
        log: L,
    ) -> Result<EGLContext, Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(log.into()).new(o!("smithay_module" => "backend_egl"));

        let (pixel_format, config_id) = match config {
            Some((attributes, reqs)) => {
                let (format, config_id) = display.choose_config(attributes, reqs)?;
                (Some(format), config_id)
            }
            None => {
                if !display
                    .extensions()
                    .iter()
                    .any(|x| x == "EGL_KHR_no_config_context")
                    && !display
                        .extensions()
                        .iter()
                        .any(|x| x == "EGL_MESA_configless_context")
                    && !display
                        .extensions()
                        .iter()
                        .any(|x| x == "EGL_KHR_surfaceless_context")
                {
                    return Err(Error::EglExtensionNotSupported(&[
                        "EGL_KHR_no_config_context",
                        "EGL_MESA_configless_context",
                        "EGL_KHR_surfaceless_context",
                    ]));
                }
                (None, ffi::egl::NO_CONFIG_KHR)
            }
        };

        let mut context_attributes = Vec::with_capacity(10);

        if let Some((attributes, _)) = config {
            let version = attributes.version;

            if display.get_egl_version() >= (1, 5)
                || display.extensions().iter().any(|s| s == "EGL_KHR_create_context")
            {
                trace!(log, "Setting CONTEXT_MAJOR_VERSION to {}", version.0);
                context_attributes.push(ffi::egl::CONTEXT_MAJOR_VERSION as i32);
                context_attributes.push(version.0 as i32);
                trace!(log, "Setting CONTEXT_MINOR_VERSION to {}", version.1);
                context_attributes.push(ffi::egl::CONTEXT_MINOR_VERSION as i32);
                context_attributes.push(version.1 as i32);

                if attributes.debug && display.get_egl_version() >= (1, 5) {
                    trace!(log, "Setting CONTEXT_OPENGL_DEBUG to TRUE");
                    context_attributes.push(ffi::egl::CONTEXT_OPENGL_DEBUG as i32);
                    context_attributes.push(ffi::egl::TRUE as i32);
                }

                context_attributes.push(ffi::egl::CONTEXT_FLAGS_KHR as i32);
                context_attributes.push(0);
            } else if display.get_egl_version() >= (1, 3) {
                trace!(log, "Setting CONTEXT_CLIENT_VERSION to {}", version.0);
                context_attributes.push(ffi::egl::CONTEXT_CLIENT_VERSION as i32);
                context_attributes.push(version.0 as i32);
            }
        } else {
            trace!(log, "Setting CONTEXT_CLIENT_VERSION to 2");
            context_attributes.push(ffi::egl::CONTEXT_CLIENT_VERSION as i32);
            context_attributes.push(2);
        }

        context_attributes.push(ffi::egl::NONE as i32);

        trace!(log, "Creating EGL context...");
        let context = wrap_egl_call(|| unsafe {
            ffi::egl::CreateContext(
                **display.get_display_handle(),
                config_id,
                shared
                    .map(|context| context.context)
                    .unwrap_or(ffi::egl::NO_CONTEXT),
                context_attributes.as_ptr(),
            )
        })
        .map_err(Error::CreationFailed)?;

        info!(log, "EGL context created");

        Ok(EGLContext {
            context,
            display: display.clone(),
            config_id,
            pixel_format,
            user_data: if let Some(shared) = shared {
                shared.user_data.clone()
            } else {
                Arc::new(UserDataMap::default())
            },
            externally_managed: false,
        })
    }

    /// Makes the OpenGL context the current context in the current thread with no surface bound.
    ///
    /// # Safety
    ///
    /// This function is marked unsafe, because the context cannot be made current on another thread without
    /// being unbound again (see [`EGLContext::unbind`]).
    pub unsafe fn make_current(&self) -> Result<(), MakeCurrentError> {
        self.make_current_with_draw_and_read_surface(None, None)
    }

    /// Makes the OpenGL context the current context in the current thread with a surface to
    /// read/draw to.
    ///
    /// # Safety
    ///
    /// This function is marked unsafe, because the context cannot be made current on another thread without
    /// being unbound again (see [`EGLContext::unbind`]).
    pub unsafe fn make_current_with_surface(&self, surface: &EGLSurface) -> Result<(), MakeCurrentError> {
        self.make_current_with_draw_and_read_surface(Some(surface), Some(surface))
    }

    /// Makes the OpenGL context the current context in the current thread with surfaces to
    /// read/draw to.
    ///
    /// # Safety
    ///
    /// This function is marked unsafe, because the context cannot be made current on another thread without
    /// being unbound again (see [`EGLContext::unbind`]).
    pub unsafe fn make_current_with_draw_and_read_surface(
        &self,
        draw_surface: Option<&EGLSurface>,
        read_surface: Option<&EGLSurface>,
    ) -> Result<(), MakeCurrentError> {
        let draw_surface_ptr = draw_surface
            .map(|s| s.surface.load(Ordering::SeqCst))
            .unwrap_or(ffi::egl::NO_SURFACE as _);
        let read_surface_ptr = read_surface
            .map(|s| s.surface.load(Ordering::SeqCst))
            .unwrap_or(ffi::egl::NO_SURFACE as _);
        wrap_egl_call(|| {
            ffi::egl::MakeCurrent(
                **self.display.get_display_handle(),
                draw_surface_ptr,
                read_surface_ptr,
                self.context,
            )
        })
        .map(|_| ())
        .map_err(Into::into)
    }

    /// Returns true if the OpenGL context is the current one in the thread.
    pub fn is_current(&self) -> bool {
        unsafe { ffi::egl::GetCurrentContext() == self.context as *const _ }
    }

    /// Returns the egl config for this context
    pub fn config_id(&self) -> ffi::egl::types::EGLConfig {
        self.config_id
    }

    /// Returns the pixel format of the main framebuffer of the context.
    pub fn pixel_format(&self) -> Option<PixelFormat> {
        self.pixel_format
    }

    /// Unbinds this context from the current thread, if set.
    ///
    /// This does nothing if this context is not the current context.
    pub fn unbind(&self) -> Result<(), MakeCurrentError> {
        if self.is_current() {
            wrap_egl_call(|| unsafe {
                ffi::egl::MakeCurrent(
                    **self.display.get_display_handle(),
                    ffi::egl::NO_SURFACE,
                    ffi::egl::NO_SURFACE,
                    ffi::egl::NO_CONTEXT,
                )
            })?;
        }
        Ok(())
    }

    /// Returns the display which created this context.
    pub fn display(&self) -> &EGLDisplay {
        &self.display
    }

    /// Returns a list of formats for dmabufs that can be rendered to.
    pub fn dmabuf_render_formats(&self) -> &HashSet<DrmFormat> {
        self.display.dmabuf_render_formats()
    }

    /// Returns a list of formats for dmabufs that can be used as textures.
    pub fn dmabuf_texture_formats(&self) -> &HashSet<DrmFormat> {
        self.display.dmabuf_texture_formats()
    }

    /// Retrieve user_data associated with this context
    ///
    /// *Note:* UserData is shared between shared context, if constructed with
    /// [`new_shared`](EGLContext::new_shared) or [`new_shared_with_config`](EGLContext::new_shared_with_config).
    pub fn user_data(&self) -> &UserDataMap {
        &self.user_data
    }

    /// Get a raw handle to the underlying context.
    ///
    /// The pointer will become invalid, when this struct is destroyed.
    pub fn get_context_handle(&self) -> ffi::egl::types::EGLContext {
        self.context
    }
}

impl Drop for EGLContext {
    fn drop(&mut self) {
        if !self.externally_managed {
            unsafe {
                // We need to ensure the context is unbound, otherwise it egl stalls the destroy call
                // ignore failures at this point
                let _ = self.unbind();
                ffi::egl::DestroyContext(**self.display.get_display_handle(), self.context);
            }
        }
    }
}

/// Attributes to use when creating an OpenGL context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlAttributes {
    /// Describes the OpenGL API and version that are being requested when a context is created.
    ///
    /// `(3, 0)` will request a OpenGL ES 3.0 context for example.
    /// `(2, 0)` is the minimum.
    pub version: (u8, u8),
    /// OpenGL profile to use.
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
    /// Include all the immediate functions and definitions.
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
    /// Using floating points allows you to write values outside the `[0.0, 1.0]` range.
    pub float_color_buffer: bool,
    /// Minimum number of bits for the alpha in the color buffer. `None` means "don't care". The default is `None`.
    pub alpha_bits: Option<u8>,
    /// Minimum number of bits for the depth buffer. `None` means "don't care". The default value is `None`.
    pub depth_bits: Option<u8>,
    /// Minimum number of bits for the depth buffer. `None` means "don't care". The default value is `None`.
    pub stencil_bits: Option<u8>,
    /// Contains the minimum number of samples per pixel in the color, depth and stencil buffers.
    /// `None` means "don't care". Default is `None`. A value of `Some(0)` indicates that multisampling must not be enabled.
    pub multisampling: Option<u16>,
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
            multisampling: None,
        }
    }
}

impl PixelFormatRequirements {
    /// Append the requirements to the given attribute list
    pub fn create_attributes(&self, out: &mut Vec<c_int>, logger: &slog::Logger) {
        if let Some(hardware_accelerated) = self.hardware_accelerated {
            out.push(ffi::egl::CONFIG_CAVEAT as c_int);
            out.push(if hardware_accelerated {
                trace!(logger, "Setting CONFIG_CAVEAT to NONE");
                ffi::egl::NONE as c_int
            } else {
                trace!(logger, "Setting CONFIG_CAVEAT to SLOW_CONFIG");
                ffi::egl::SLOW_CONFIG as c_int
            });
        }

        if let Some(color) = self.color_bits {
            trace!(logger, "Setting RED_SIZE to {}", color / 3);
            out.push(ffi::egl::RED_SIZE as c_int);
            out.push((color / 3) as c_int);
            trace!(
                logger,
                "Setting GREEN_SIZE to {}",
                color / 3 + u8::from(color % 3 != 0)
            );
            out.push(ffi::egl::GREEN_SIZE as c_int);
            out.push((color / 3 + u8::from(color % 3 != 0)) as c_int);
            trace!(
                logger,
                "Setting BLUE_SIZE to {}",
                color / 3 + u8::from(color % 3 == 2)
            );
            out.push(ffi::egl::BLUE_SIZE as c_int);
            out.push((color / 3 + u8::from(color % 3 == 2)) as c_int);
        }

        if let Some(alpha) = self.alpha_bits {
            trace!(logger, "Setting ALPHA_SIZE to {}", alpha);
            out.push(ffi::egl::ALPHA_SIZE as c_int);
            out.push(alpha as c_int);
        }

        if let Some(depth) = self.depth_bits {
            trace!(logger, "Setting DEPTH_SIZE to {}", depth);
            out.push(ffi::egl::DEPTH_SIZE as c_int);
            out.push(depth as c_int);
        }

        if let Some(stencil) = self.stencil_bits {
            trace!(logger, "Setting STENCIL_SIZE to {}", stencil);
            out.push(ffi::egl::STENCIL_SIZE as c_int);
            out.push(stencil as c_int);
        }

        if let Some(multisampling) = self.multisampling {
            trace!(logger, "Setting SAMPLES to {}", multisampling);
            out.push(ffi::egl::SAMPLES as c_int);
            out.push(multisampling as c_int);
        }
    }
}
