//! EGL surface related structs

use std::fmt;
use std::sync::{
    atomic::{AtomicPtr, Ordering},
    Arc,
};

use crate::backend::egl::{
    display::{DamageSupport, EGLDisplay, EGLDisplayHandle, PixelFormat},
    ffi,
    native::EGLNativeSurface,
    EGLError, SwapBuffersError,
};
use crate::utils::{Physical, Rectangle, Size};

use tracing::{debug, info_span, instrument};

/// EGL surface of a given EGL context for rendering
pub struct EGLSurface {
    pub(crate) display: Arc<EGLDisplayHandle>,
    native: Box<dyn EGLNativeSurface + Send + 'static>,
    pub(crate) surface: AtomicPtr<std::ffi::c_void>,
    config_id: ffi::egl::types::EGLConfig,
    pixel_format: PixelFormat,
    damage_impl: DamageSupport,
    span: tracing::Span,
}

impl fmt::Debug for EGLSurface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EGLSurface")
            .field("display", &self.display)
            // native does not necessarily implement Debug
            .field("surface", &self.surface)
            .field("config_id", &self.config_id)
            .field("pixel_format", &self.pixel_format)
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
    ///
    /// # Safety
    ///
    /// - `config_id` has to represent a valid config
    pub unsafe fn new<N>(
        display: &EGLDisplay,
        pixel_format: PixelFormat,
        config: ffi::egl::types::EGLConfig,
        native: N,
    ) -> Result<EGLSurface, EGLError>
    where
        N: EGLNativeSurface + Send + 'static,
    {
        let span = info_span!(
            parent: &display.span,
            "egl_surface",
            native = tracing::field::Empty
        );
        if let Some(value) = native.identifier() {
            span.record("native", value);
        }

        let surface = unsafe { native.create(&display.get_display_handle(), config)? };
        if surface == ffi::egl::NO_SURFACE {
            return Err(EGLError::BadSurface);
        }

        Ok(EGLSurface {
            display: display.get_display_handle(),
            native: Box::new(native),
            surface: AtomicPtr::new(surface as *mut _),
            config_id: config,
            pixel_format,
            damage_impl: display.supports_damage_impl(),
            span,
        })
    }

    /// Returns the buffer age of the underlying back buffer
    #[profiling::function]
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
            debug!(
                "Failed to query buffer age value for surface {:?}: {}",
                self,
                EGLError::from_last_call().unwrap_or_else(|| {
                    tracing::warn!("Erroneous EGL call didn't set EGLError");
                    EGLError::Unknown(0)
                })
            );
            None
        } else {
            Some(age)
        }
    }

    /// Returns the size of the underlying back buffer
    #[profiling::function]
    pub fn get_size(&self) -> Option<Size<i32, Physical>> {
        let surface = self.surface.load(Ordering::SeqCst);
        let mut height = 0;
        let ret_h = unsafe {
            ffi::egl::QuerySurface(
                **self.display,
                surface as *const _,
                ffi::egl::HEIGHT as i32,
                &mut height as *mut _,
            )
        };
        let mut width = 0;
        let ret_w = unsafe {
            ffi::egl::QuerySurface(
                **self.display,
                surface as *const _,
                ffi::egl::WIDTH as i32,
                &mut width as *mut _,
            )
        };
        if ret_h == ffi::egl::FALSE || ret_w == ffi::egl::FALSE {
            debug!(
                parent: &self.span,
                "Failed to query size value for surface {:?}: {}",
                self,
                EGLError::from_last_call().unwrap_or_else(|| {
                    tracing::warn!("Erroneous EGL call didn't set EGLError");
                    EGLError::Unknown(0)
                })
            );
            None
        } else {
            Some(Size::from((width, height)))
        }
    }

    /// Swaps buffers at the end of a frame.
    #[instrument(level = "trace", parent = &self.span, skip(self), err)]
    #[profiling::function]
    pub fn swap_buffers(
        &self,
        damage: Option<&mut [Rectangle<i32, Physical>]>,
    ) -> ::std::result::Result<(), SwapBuffersError> {
        let surface = self.surface.load(Ordering::SeqCst);

        let result = if !surface.is_null() {
            self.native
                .swap_buffers(&self.display, surface, damage, self.damage_impl)
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
                    unsafe {
                        self.native
                            .create(&self.display, self.config_id)
                            .map_err(SwapBuffersError::EGLCreateSurface)? as *mut _
                    },
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
                debug!("Hiding page-flip error *before* recreation: {}", err);
                SwapBuffersError::EGLSwapBuffers(EGLError::BadSurface)
            })
        } else {
            result
        }
    }

    /// Returns true if the OpenGL surface is the current one in the thread.
    #[profiling::function]
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
