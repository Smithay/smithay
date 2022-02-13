//! EGL surface related structs

use std::fmt;
use std::sync::{
    atomic::{AtomicPtr, Ordering},
    Arc,
};

use crate::backend::egl::{
    display::{EGLDisplay, EGLDisplayHandle, PixelFormat},
    ffi,
    native::EGLNativeSurface,
    EGLError, SwapBuffersError,
};
use crate::utils::{Physical, Rectangle};

use slog::{debug, o};

/// EGL surface of a given EGL context for rendering
pub struct EGLSurface {
    pub(crate) display: Arc<EGLDisplayHandle>,
    native: Box<dyn EGLNativeSurface + Send + 'static>,
    pub(crate) surface: AtomicPtr<nix::libc::c_void>,
    config_id: ffi::egl::types::EGLConfig,
    pixel_format: PixelFormat,
    logger: ::slog::Logger,
}

impl fmt::Debug for EGLSurface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EGLSurface")
            .field("display", &self.display)
            // native does not necessarily implement Debug
            .field("surface", &self.surface)
            .field("config_id", &self.config_id)
            .field("pixel_format", &self.pixel_format)
            .field("logger", &self.logger)
            .finish()
    }
}

// safe because EGLConfig can be moved between threads
// and the other types are thread-safe
unsafe impl Send for EGLSurface {}

impl EGLSurface {
    /// Create a new `EGLSurface`.
    ///
    /// Requires:
    /// - A EGLDisplay supported by the corresponding platform matching the surface type
    /// - A pixel format
    /// - A valid `EGLConfig` (see `EGLContext::config_id()`)
    /// - A native type backing the surface matching the used platform
    /// - An (optional) Logger
    pub fn new<N, L>(
        display: &EGLDisplay,
        pixel_format: PixelFormat,
        config: ffi::egl::types::EGLConfig,
        native: N,
        log: L,
    ) -> Result<EGLSurface, EGLError>
    where
        N: EGLNativeSurface + Send + 'static,
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(log.into()).new(o!("smithay_module" => "renderer_egl"));

        let surface = native.create(&display.display, config)?;
        if surface == ffi::egl::NO_SURFACE {
            return Err(EGLError::BadSurface);
        }

        Ok(EGLSurface {
            display: display.display.clone(),
            native: Box::new(native),
            surface: AtomicPtr::new(surface as *mut _),
            config_id: config,
            pixel_format,
            logger: log,
        })
    }

    /// Returns the buffer age of the underlying back buffer
    pub fn buffer_age(&self) -> Option<i32> {
        let surface = self.surface.load(Ordering::SeqCst);
        let mut age = 0;
        let ret = unsafe {
            ffi::egl::QuerySurface(
                **self.display,
                surface as *const _,
                ffi::egl::BUFFER_AGE_EXT as i32,
                &mut age as *mut _,
            )
        };
        if ret == ffi::egl::FALSE {
            slog::debug!(
                self.logger,
                "Failed to query buffer age value for surface {:?}: {}",
                self,
                EGLError::from_last_call().unwrap_err()
            );
            None
        } else {
            Some(age)
        }
    }

    /// Swaps buffers at the end of a frame.
    pub fn swap_buffers(
        &self,
        damage: Option<&mut [Rectangle<i32, Physical>]>,
    ) -> ::std::result::Result<(), SwapBuffersError> {
        let surface = self.surface.load(Ordering::SeqCst);

        let result = if !surface.is_null() {
            self.native.swap_buffers(&self.display, surface, damage)
        } else {
            Err(SwapBuffersError::EGLSwapBuffers(EGLError::BadSurface))
        };

        // workaround for missing `PartialEq` impl
        let is_bad_surface = matches!(
            result,
            Err(SwapBuffersError::EGLSwapBuffers(EGLError::BadSurface))
        );

        if self.native.needs_recreation() || surface.is_null() || is_bad_surface {
            let previous = self
                .surface
                .compare_exchange(
                    surface,
                    self.native
                        .create(&self.display, self.config_id)
                        .map_err(SwapBuffersError::EGLCreateSurface)? as *mut _,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .expect("The surface pointer changed in between?");
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

    /// Get a raw handle to the underlying surface
    ///
    /// *Note*: The surface might get dynamically recreated during swap-buffers
    /// causing the pointer to become invalid.
    ///
    /// The pointer will become invalid, when this struct is destroyed.
    pub fn get_surface_handle(&self) -> ffi::egl::types::EGLSurface {
        self.surface.load(Ordering::SeqCst)
    }
}

impl Drop for EGLSurface {
    fn drop(&mut self) {
        unsafe {
            ffi::egl::DestroySurface(**self.display, *self.surface.get_mut() as *const _);
        }
    }
}
