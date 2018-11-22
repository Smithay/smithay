//! EGL surface related structs

use super::{error::*, ffi, native, EGLContext};
use backend::graphics::SwapBuffersError;
use nix::libc::c_int;
use std::{
    cell::Cell,
    ops::{Deref, DerefMut},
    rc::{Rc, Weak},
};

/// EGL surface of a given EGL context for rendering
pub struct EGLSurface<N: native::NativeSurface> {
    context: Weak<ffi::egl::types::EGLContext>,
    display: Weak<ffi::egl::types::EGLDisplay>,
    native: N,
    surface: Cell<ffi::egl::types::EGLSurface>,
    config_id: ffi::egl::types::EGLConfig,
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
    pub(crate) fn new<B: native::Backend<Surface = N>, D: native::NativeDisplay<B>>(
        context: &EGLContext<B, D>,
        native: N,
    ) -> Result<EGLSurface<N>> {
        let surface = unsafe {
            ffi::egl::CreateWindowSurface(
                *context.display,
                context.config_id,
                native.ptr(),
                context.surface_attributes.as_ptr(),
            )
        };

        if surface.is_null() {
            bail!(ErrorKind::SurfaceCreationFailed);
        }

        Ok(EGLSurface {
            context: Rc::downgrade(&context.context),
            display: Rc::downgrade(&context.display),
            native,
            surface: Cell::new(surface),
            config_id: context.config_id,
            surface_attributes: context.surface_attributes.clone(),
        })
    }

    /// Swaps buffers at the end of a frame.
    pub fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError> {
        let surface = self.surface.get();

        if !surface.is_null() {
            if let Some(display) = self.display.upgrade() {
                let ret = unsafe { ffi::egl::SwapBuffers((*display) as *const _, surface as *const _) };

                if ret == 0 {
                    match unsafe { ffi::egl::GetError() } as u32 {
                        ffi::egl::CONTEXT_LOST => return Err(SwapBuffersError::ContextLost),
                        err => return Err(SwapBuffersError::Unknown(err)),
                    };
                } else {
                    return Ok(());
                }
            } else {
                return Err(SwapBuffersError::ContextLost);
            }
            self.native.swap_buffers()?;
        };

        if self.native.needs_recreation() || surface.is_null() {
            if let Some(display) = self.display.upgrade() {
                self.native.recreate();
                self.surface.set(unsafe {
                    ffi::egl::CreateWindowSurface(
                        *display,
                        self.config_id,
                        self.native.ptr(),
                        self.surface_attributes.as_ptr(),
                    )
                });
            }
        }

        Ok(())
    }

    /// Makes the OpenGL context the current context in the current thread.
    ///
    /// # Unsafety
    ///
    /// This function is marked unsafe, because the context cannot be made current
    /// on multiple threads.
    pub unsafe fn make_current(&self) -> ::std::result::Result<(), SwapBuffersError> {
        if let (Some(display), Some(context)) = (self.display.upgrade(), self.context.upgrade()) {
            let ret = ffi::egl::MakeCurrent(
                (*display) as *const _,
                self.surface.get() as *const _,
                self.surface.get() as *const _,
                (*context) as *const _,
            );

            if ret == 0 {
                match ffi::egl::GetError() as u32 {
                    ffi::egl::CONTEXT_LOST => Err(SwapBuffersError::ContextLost),
                    err => panic!("eglMakeCurrent failed (eglGetError returned 0x{:x})", err),
                }
            } else {
                Ok(())
            }
        } else {
            Err(SwapBuffersError::ContextLost)
        }
    }

    /// Returns true if the OpenGL surface is the current one in the thread.
    pub fn is_current(&self) -> bool {
        if self.context.upgrade().is_some() {
            unsafe {
                ffi::egl::GetCurrentSurface(ffi::egl::DRAW as _) == self.surface.get() as *const _
                    && ffi::egl::GetCurrentSurface(ffi::egl::READ as _) == self.surface.get() as *const _
            }
        } else {
            false
        }
    }
}

impl<N: native::NativeSurface> Drop for EGLSurface<N> {
    fn drop(&mut self) {
        if let Some(display) = self.display.upgrade() {
            unsafe {
                ffi::egl::DestroySurface((*display) as *const _, self.surface.get() as *const _);
            }
        }
    }
}
