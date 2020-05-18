//! EGL surface related structs

use super::{ffi, native, EGLError, SurfaceCreationError, SwapBuffersError};
use crate::backend::egl::display::EGLDisplayHandle;
use crate::backend::graphics::PixelFormat;
use nix::libc::c_int;
use std::sync::Arc;
use std::{
    cell::Cell,
    ops::{Deref, DerefMut},
};

/// EGL surface of a given EGL context for rendering
pub struct EGLSurface<N: native::NativeSurface> {
    pub(crate) display: Arc<EGLDisplayHandle>,
    native: N,
    pub(crate) surface: Cell<ffi::egl::types::EGLSurface>,
    config_id: ffi::egl::types::EGLConfig,
    pixel_format: PixelFormat,
    surface_attributes: Vec<c_int>,
    logger: ::slog::Logger,
}

impl<N: native::NativeSurface> Deref for EGLSurface<N> {
    type Target = N;
    fn deref(&self) -> &N {
        &self.native
    }
}

impl<N: native::NativeSurface> DerefMut for EGLSurface<N> {
    fn deref_mut(&mut self) -> &mut N {
        &mut self.native
    }
}

impl<N: native::NativeSurface> EGLSurface<N> {
    pub(crate) fn new<L>(
        display: Arc<EGLDisplayHandle>,
        pixel_format: PixelFormat,
        double_buffered: Option<bool>,
        config: ffi::egl::types::EGLConfig,
        native: N,
        log: L,
    ) -> Result<EGLSurface<N>, SurfaceCreationError<N::Error>>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(log.into()).new(o!("smithay_module" => "renderer_egl"));

        let surface_attributes = {
            let mut out: Vec<c_int> = Vec::with_capacity(3);

            match double_buffered {
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

        let surface = unsafe { native.create(&display, config, &surface_attributes)? };

        if surface == ffi::egl::NO_SURFACE {
            return Err(SurfaceCreationError::EGLSurfaceCreationFailed(
                EGLError::BadSurface,
            ));
        }

        Ok(EGLSurface {
            display,
            native,
            surface: Cell::new(surface),
            config_id: config,
            pixel_format,
            surface_attributes,
            logger: log,
        })
    }

    /// Swaps buffers at the end of a frame.
    pub fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError<N::Error>> {
        let surface = self.surface.get();

        let result = if !surface.is_null() {
            self.native.swap_buffers(&self.display, surface)
        } else {
            Err(SwapBuffersError::EGLSwapBuffers(EGLError::BadSurface))
        };

        // workaround for missing `PartialEq` impl
        let is_bad_surface = if let Err(SwapBuffersError::EGLSwapBuffers(EGLError::BadSurface)) = result {
            true
        } else {
            false
        };

        if self.native.needs_recreation() || surface.is_null() || is_bad_surface {
            if !surface.is_null() {
                let _ = unsafe { ffi::egl::DestroySurface(**self.display, surface as *const _) };
            }
            self.surface.set(unsafe {
                self.native
                    .create(&self.display, self.config_id, &self.surface_attributes)
                    .map_err(|err| match err {
                        SurfaceCreationError::EGLSurfaceCreationFailed(err) => {
                            SwapBuffersError::EGLCreateWindowSurface(err)
                        }
                        SurfaceCreationError::NativeSurfaceCreationFailed(err) => {
                            SwapBuffersError::Underlying(err)
                        }
                    })?
            });

            result.map_err(|err| {
                debug!(self.logger, "Hiding page-flip error *before* recreation: {}", err);
                SwapBuffersError::EGLSwapBuffers(EGLError::BadSurface)
            })
        } else {
            result
        }
    }

    /// Returns true if the OpenGL surface is the current one in the thread.
    pub fn is_current(&self) -> bool {
        unsafe {
            ffi::egl::GetCurrentSurface(ffi::egl::DRAW as _) == self.surface.get() as *const _
                && ffi::egl::GetCurrentSurface(ffi::egl::READ as _) == self.surface.get() as *const _
        }
    }

    /// Returns the egl config for this context
    pub fn get_config_id(&self) -> ffi::egl::types::EGLConfig {
        self.config_id
    }

    /// Returns the pixel format of the main framebuffer of the context.
    pub fn get_pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }
}

impl<N: native::NativeSurface> Drop for EGLSurface<N> {
    fn drop(&mut self) {
        unsafe {
            ffi::egl::DestroySurface(**self.display, self.surface.get() as *const _);
        }
    }
}
