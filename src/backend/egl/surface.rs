//! EGL surface related structs

use std::sync::{
    atomic::{AtomicPtr, Ordering},
    Arc,
};

use nix::libc::c_int;

use crate::backend::egl::{
    display::{EGLDisplay, EGLDisplayHandle, PixelFormat},
    native::EGLNativeSurface,
    ffi, EGLError, SwapBuffersError
};


/// EGL surface of a given EGL context for rendering
pub struct EGLSurface {
    pub(crate) display: Arc<EGLDisplayHandle>,
    native: Box<dyn EGLNativeSurface + Send + 'static>,
    pub(crate) surface: AtomicPtr<nix::libc::c_void>,
    config_id: ffi::egl::types::EGLConfig,
    pixel_format: PixelFormat,
    surface_attributes: Vec<c_int>,
    logger: ::slog::Logger,
}
// safe because EGLConfig can be moved between threads
// and the other types are thread-safe
unsafe impl Send for EGLSurface {}

impl EGLSurface {
    pub fn new<N, L>(
        display: &EGLDisplay,
        pixel_format: PixelFormat,
        double_buffered: Option<bool>,
        config: ffi::egl::types::EGLConfig,
        native: N,
        log: L,
    ) -> Result<EGLSurface, EGLError>
    where
        N: EGLNativeSurface + Send + 'static,
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

        let surface = native.create(&display.display, config, &surface_attributes)?;

        if surface == ffi::egl::NO_SURFACE {
            return Err(EGLError::BadSurface);
        }

        Ok(EGLSurface {
            display: display.display.clone(),
            native: Box::new(native),
            surface: AtomicPtr::new(surface as *mut _),
            config_id: config,
            pixel_format,
            surface_attributes,
            logger: log,
        })
    }

    /// Swaps buffers at the end of a frame.
    pub fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError> {
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
            let previous = self.surface.compare_exchange(
                surface,
                self.native
                    .create(&self.display, self.config_id, &self.surface_attributes)
                    .map_err(SwapBuffersError::EGLCreateSurface)? as *mut _,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ).expect("The surface pointer changed in between?");
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
    pub fn config_id(&self) -> ffi::egl::types::EGLConfig {
        self.config_id
    }

    /// Returns the pixel format of the main framebuffer of the context.
    pub fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }
    
    /// Tries to resize the underlying native surface.
    /// 
    /// The two first arguments (width, height) are the new size of the surface,
    /// the two others (dx, dy) represent the displacement of the top-left corner of the surface.
    /// It allows you to control the direction of the resizing if necessary.
    /// 
    /// Implementations may ignore the dx and dy arguments.
    /// 
    /// Returns true if the resize was successful.
    pub fn resize(&self, width: i32, height: i32, dx: i32, dy: i32) -> bool {
        self.native.resize(width, height, dx, dy)
    }
}

impl Drop for EGLSurface {
    fn drop(&mut self) {
        unsafe {
            ffi::egl::DestroySurface(**self.display, *self.surface.get_mut() as *const _);
        }
    }
}
