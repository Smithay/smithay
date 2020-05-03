//! EGL surface related structs

use super::{ffi, native, wrap_egl_call, EGLError, SwapBuffersError};
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
    ) -> Result<EGLSurface<N>, EGLError>
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

        let surface = wrap_egl_call(|| unsafe {
            ffi::egl::CreateWindowSurface(**display, config, native.ptr(), surface_attributes.as_ptr())
        })?;

        Ok(EGLSurface {
            display,
            native,
            surface: Cell::new(surface),
            config_id: config,
            pixel_format,
            surface_attributes,
        })
    }

    /// Swaps buffers at the end of a frame.
    pub fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError<N::Error>> {
        let surface = self.surface.get();

        let result = if !surface.is_null() {
            wrap_egl_call(|| unsafe { ffi::egl::SwapBuffers(**self.display, surface as *const _) })
                .map_err(SwapBuffersError::EGLSwapBuffers)
                .and_then(|_| self.native.swap_buffers().map_err(SwapBuffersError::Underlying))
        } else { Err(SwapBuffersError::EGLSwapBuffers(EGLError::BadSurface)) };

        // workaround for missing `PartialEq` impl
        let is_bad_surface = if let Err(SwapBuffersError::EGLSwapBuffers(EGLError::BadSurface)) = result {
            true
        } else {
            false
        };

        if self.native.needs_recreation() || surface.is_null() || is_bad_surface {
            self.native.recreate().map_err(SwapBuffersError::Underlying)?;
            if !surface.is_null() {
                let _ = unsafe { ffi::egl::DestroySurface(**self.display, surface as *const _) };
            }
            self.surface.set(unsafe {
                wrap_egl_call(|| {
                    ffi::egl::CreateWindowSurface(
                        **self.display,
                        self.config_id,
                        self.native.ptr(),
                        self.surface_attributes.as_ptr(),
                    )
                })
                .map_err(SwapBuffersError::EGLCreateWindowSurface)?
            });

            result.map_err(|_| SwapBuffersError::EGLSwapBuffers(EGLError::BadSurface))
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
