//! EGL surface related structs

use super::{ffi, native, EGLError, SurfaceCreationError, SwapBuffersError};
use crate::backend::egl::display::EGLDisplayHandle;
use crate::backend::graphics::PixelFormat;
use nix::libc::c_int;
use std::ops::{Deref, DerefMut};
use std::sync::{
    atomic::{AtomicPtr, Ordering},
    Arc,
};

/// EGL surface of a given EGL context for rendering
pub struct EGLSurface<N: native::NativeSurface> {
    pub(crate) display: Arc<EGLDisplayHandle>,
    native: N,
    pub(crate) surface: AtomicPtr<nix::libc::c_void>,
    config_id: ffi::egl::types::EGLConfig,
    pixel_format: PixelFormat,
    surface_attributes: Vec<c_int>,
    logger: ::slog::Logger,
}
// safe because EGLConfig can be moved between threads
// and the other types are thread-safe
unsafe impl<N: native::NativeSurface + Send> Send for EGLSurface<N> {}
unsafe impl<N: native::NativeSurface + Send + Sync> Sync for EGLSurface<N> {}

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
        let log = crate::slog_or_fallback(log.into()).new(o!("smithay_module" => "renderer_egl"));

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
            surface: AtomicPtr::new(surface as *mut _),
            config_id: config,
            pixel_format,
            surface_attributes,
            logger: log,
        })
    }

    /// Swaps buffers at the end of a frame.
    pub fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError<N::Error>> {
        let surface = self.surface.load(Ordering::SeqCst);

        let result = if !surface.is_null() {
            self.native.swap_buffers(&self.display, surface)
        } else {
            Err(SwapBuffersError::EGLSwapBuffers(EGLError::BadSurface))
        };

        // workaround for missing `PartialEq` impl
        let is_bad_surface = matches!(
            result,
            Err(SwapBuffersError::EGLSwapBuffers(EGLError::BadSurface))
        );

        if self.native.needs_recreation() || surface.is_null() || is_bad_surface {
            let previous = self.surface.compare_and_swap(
                surface,
                unsafe {
                    self.native
                        .create(&self.display, self.config_id, &self.surface_attributes)
                        .map_err(|err| match err {
                            SurfaceCreationError::EGLSurfaceCreationFailed(err) => {
                                SwapBuffersError::EGLCreateWindowSurface(err)
                            }
                            SurfaceCreationError::NativeSurfaceCreationFailed(err) => {
                                SwapBuffersError::Underlying(err)
                            }
                        })? as *mut _
                },
                Ordering::SeqCst,
            );
            if previous == surface && !surface.is_null() {
                let _ = unsafe { ffi::egl::DestroySurface(**self.display, surface as *const _) };
            }

            // if a recreation is pending anyway, ignore page-flip errors.
            // lets see if we still fail after the next commit.
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
        let surface = self.surface.load(Ordering::SeqCst);
        unsafe {
            ffi::egl::GetCurrentSurface(ffi::egl::DRAW as _) == surface as *const _
                && ffi::egl::GetCurrentSurface(ffi::egl::READ as _) == surface as *const _
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
            ffi::egl::DestroySurface(**self.display, *self.surface.get_mut() as *const _);
        }
    }
}
