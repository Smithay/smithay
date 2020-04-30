//! EGL context related structs

use super::{ffi, wrap_egl_call, Error, MakeCurrentError};
use crate::backend::egl::display::{EGLDisplay, EGLDisplayHandle};
use crate::backend::egl::native::NativeSurface;
use crate::backend::egl::{native, EGLSurface};
use crate::backend::graphics::PixelFormat;
use std::os::raw::c_int;
use std::ptr;
use std::sync::Arc;

/// EGL context for rendering
pub struct EGLContext {
    context: ffi::egl::types::EGLContext,
    display: Arc<EGLDisplayHandle>,
    config_id: ffi::egl::types::EGLConfig,
    pixel_format: PixelFormat,
}

impl EGLContext {
    /// Create a new [`EGLContext`] from a given [`NativeDisplay`](native::NativeDisplay)
    pub(crate) fn new<B, N, L>(
        display: &EGLDisplay<B, N>,
        mut attributes: GlAttributes,
        reqs: PixelFormatRequirements,
        log: L,
    ) -> Result<EGLContext, Error>
    where
        L: Into<Option<::slog::Logger>>,
        B: native::Backend,
        N: native::NativeDisplay<B>,
    {
        let log = crate::slog_or_stdlog(log.into()).new(o!("smithay_module" => "renderer_egl"));

        // If no version is given, try OpenGLES 3.0, if available,
        // fallback to 2.0 otherwise
        let version = match attributes.version {
            Some((3, x)) => (3, x),
            Some((2, x)) => (2, x),
            None => {
                debug!(log, "Trying to initialize EGL with OpenGLES 3.0");
                attributes.version = Some((3, 0));
                match EGLContext::new(display, attributes, reqs, log.clone()) {
                    Ok(x) => return Ok(x),
                    Err(err) => {
                        warn!(log, "EGL OpenGLES 3.0 Initialization failed with {}", err);
                        debug!(log, "Trying to initialize EGL with OpenGLES 2.0");
                        attributes.version = Some((2, 0));
                        return EGLContext::new(display, attributes, reqs, log.clone());
                    }
                }
            }
            Some((1, x)) => {
                error!(log, "OpenGLES 1.* is not supported by the EGL renderer backend");
                return Err(Error::OpenGlVersionNotSupported((1, x)));
            }
            Some(version) => {
                error!(
                    log,
                    "OpenGLES {:?} is unknown and not supported by the EGL renderer backend", version
                );
                return Err(Error::OpenGlVersionNotSupported(version));
            }
        };

        let (pixel_format, config_id) = display.choose_config(attributes, reqs)?;

        let mut context_attributes = Vec::with_capacity(10);

        if display.egl_version >= (1, 5) || display.extensions.iter().any(|s| s == "EGL_KHR_create_context") {
            trace!(log, "Setting CONTEXT_MAJOR_VERSION to {}", version.0);
            context_attributes.push(ffi::egl::CONTEXT_MAJOR_VERSION as i32);
            context_attributes.push(version.0 as i32);
            trace!(log, "Setting CONTEXT_MINOR_VERSION to {}", version.1);
            context_attributes.push(ffi::egl::CONTEXT_MINOR_VERSION as i32);
            context_attributes.push(version.1 as i32);

            if attributes.debug && display.egl_version >= (1, 5) {
                trace!(log, "Setting CONTEXT_OPENGL_DEBUG to TRUE");
                context_attributes.push(ffi::egl::CONTEXT_OPENGL_DEBUG as i32);
                context_attributes.push(ffi::egl::TRUE as i32);
            }

            context_attributes.push(ffi::egl::CONTEXT_FLAGS_KHR as i32);
            context_attributes.push(0);
        } else if display.egl_version >= (1, 3) {
            trace!(log, "Setting CONTEXT_CLIENT_VERSION to {}", version.0);
            context_attributes.push(ffi::egl::CONTEXT_CLIENT_VERSION as i32);
            context_attributes.push(version.0 as i32);
        }

        context_attributes.push(ffi::egl::NONE as i32);

        trace!(log, "Creating EGL context...");
        // TODO: Support shared contexts
        let context = wrap_egl_call(|| unsafe {
            ffi::egl::CreateContext(
                **display.display,
                config_id,
                ptr::null(),
                context_attributes.as_ptr(),
            )
        })
        .map_err(Error::CreationFailed)?;

        info!(log, "EGL context created");

        Ok(EGLContext {
            context,
            display: display.display.clone(),
            config_id,
            pixel_format,
        })
    }

    /// Makes the OpenGL context the current context in the current thread with a surface to
    /// read/write to.
    ///
    /// # Safety
    ///
    /// This function is marked unsafe, because the context cannot be made current
    /// on multiple threads.
    pub unsafe fn make_current_with_surface<N>(&self, surface: &EGLSurface<N>) -> Result<(), MakeCurrentError>
    where
        N: NativeSurface,
    {
        let surface_ptr = surface.surface.get();

        wrap_egl_call(|| ffi::egl::MakeCurrent(**self.display, surface_ptr, surface_ptr, self.context))
            .map(|_| ())
            .map_err(Into::into)
    }

    /// Makes the OpenGL context the current context in the current thread with no surface bound.
    ///
    /// # Safety
    ///
    /// This function is marked unsafe, because the context cannot be made current
    /// on multiple threads without being unbound again (see `unbind`)
    pub unsafe fn make_current(&self) -> Result<(), MakeCurrentError> {
        wrap_egl_call(|| {
            ffi::egl::MakeCurrent(
                **self.display,
                ffi::egl::NO_SURFACE,
                ffi::egl::NO_SURFACE,
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
    pub fn get_config_id(&self) -> ffi::egl::types::EGLConfig {
        self.config_id
    }

    /// Returns the pixel format of the main framebuffer of the context.
    pub fn get_pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    /// Unbinds this context from the current thread, if set.
    ///
    /// This does nothing if this context is not the current context
    pub fn unbind(&self) -> Result<(), MakeCurrentError> {
        if self.is_current() {
            wrap_egl_call(|| unsafe {
                ffi::egl::MakeCurrent(
                    **self.display,
                    ffi::egl::NO_SURFACE,
                    ffi::egl::NO_SURFACE,
                    ffi::egl::NO_CONTEXT,
                )
            })?;
        }
        Ok(())
    }
}

impl Drop for EGLContext {
    fn drop(&mut self) {
        unsafe {
            // We need to ensure the context is unbound, otherwise it egl stalls the destroy call
            // ignore failures at this point
            let _ = self.unbind();
            ffi::egl::DestroyContext(**self.display, self.context);
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

impl PixelFormatRequirements {
    /// Append the  requirements to the given attribute list
    pub fn create_attributes(&self, out: &mut Vec<c_int>, logger: &slog::Logger) -> Result<(), ()> {
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
                color / 3 + if color % 3 != 0 { 1 } else { 0 }
            );
            out.push(ffi::egl::GREEN_SIZE as c_int);
            out.push((color / 3 + if color % 3 != 0 { 1 } else { 0 }) as c_int);
            trace!(
                logger,
                "Setting BLUE_SIZE to {}",
                color / 3 + if color % 3 == 2 { 1 } else { 0 }
            );
            out.push(ffi::egl::BLUE_SIZE as c_int);
            out.push((color / 3 + if color % 3 == 2 { 1 } else { 0 }) as c_int);
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

        if self.stereoscopy {
            error!(logger, "Stereoscopy is currently unsupported (sorry!)");
            return Err(());
        }

        Ok(())
    }
}
