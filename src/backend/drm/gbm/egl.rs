//!
//! Egl [`NativeDisplay`](::backend::egl::native::NativeDisplay) and
//! [`NativeSurface`](::backend::egl::native::NativeSurface) support for
//! [`GbmDevice`](GbmDevice) and [`GbmSurface`](GbmSurface).
//!

use crate::backend::drm::{Device, RawDevice, Surface};
use crate::backend::egl::native::{Backend, NativeDisplay, NativeSurface};
use crate::backend::egl::{display::EGLDisplayHandle, ffi};
use crate::backend::egl::{
    wrap_egl_call, EGLError, Error as EglBackendError, SurfaceCreationError, SwapBuffersError,
};

use super::{Error, GbmDevice, GbmSurface};

use drm::control::{connector, crtc, Device as ControlDevice, Mode};
use gbm::AsRaw;
use nix::libc::{c_int, c_void};
use std::marker::PhantomData;

/// Egl Gbm backend type
///
/// See [`Backend`](::backend::egl::native::Backend).
pub struct Gbm<D: RawDevice + 'static> {
    _userdata: PhantomData<D>,
}

impl<D: RawDevice + 'static> Backend for Gbm<D> {
    type Surface = GbmSurface<D>;
    type Error = Error<<<D as Device>::Surface as Surface>::Error>;

    unsafe fn get_display<F>(
        display: ffi::NativeDisplayType,
        attribs: &[ffi::EGLint],
        has_dp_extension: F,
        log: ::slog::Logger,
    ) -> Result<ffi::egl::types::EGLDisplay, EGLError>
    where
        F: Fn(&str) -> bool,
    {
        if has_dp_extension("EGL_KHR_platform_gbm") && ffi::egl::GetPlatformDisplay::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_KHR_platform_gbm");
            let attribs = attribs.iter().map(|x| *x as isize).collect::<Vec<_>>();
            wrap_egl_call(|| {
                ffi::egl::GetPlatformDisplay(ffi::egl::PLATFORM_GBM_KHR, display as *mut _, attribs.as_ptr())
            })
        } else if has_dp_extension("EGL_MESA_platform_gbm") && ffi::egl::GetPlatformDisplayEXT::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_MESA_platform_gbm");
            wrap_egl_call(|| {
                ffi::egl::GetPlatformDisplayEXT(
                    ffi::egl::PLATFORM_GBM_MESA,
                    display as *mut _,
                    attribs.as_ptr(),
                )
            })
        } else if has_dp_extension("EGL_MESA_platform_gbm") && ffi::egl::GetPlatformDisplay::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_MESA_platform_gbm");
            let attribs = attribs.iter().map(|x| *x as isize).collect::<Vec<_>>();
            wrap_egl_call(|| {
                ffi::egl::GetPlatformDisplay(ffi::egl::PLATFORM_GBM_MESA, display as *mut _, attribs.as_ptr())
            })
        } else {
            Ok(ffi::egl::NO_DISPLAY)
        }
    }
}

unsafe impl<D: RawDevice + ControlDevice + 'static> NativeDisplay<Gbm<D>> for GbmDevice<D> {
    type Arguments = (crtc::Handle, Mode, Vec<connector::Handle>);

    fn is_backend(&self) -> bool {
        true
    }

    fn ptr(&self) -> Result<ffi::NativeDisplayType, EglBackendError> {
        Ok(self.dev.borrow().as_raw() as *const _)
    }

    fn create_surface(
        &mut self,
        args: Self::Arguments,
    ) -> Result<GbmSurface<D>, Error<<<D as Device>::Surface as Surface>::Error>> {
        Device::create_surface(self, args.0, args.1, &args.2)
    }
}

unsafe impl<D: RawDevice + 'static> NativeSurface for GbmSurface<D> {
    type Error = Error<<<D as Device>::Surface as Surface>::Error>;

    unsafe fn create(
        &self,
        display: &EGLDisplayHandle,
        config_id: ffi::egl::types::EGLConfig,
        surface_attributes: &[c_int],
    ) -> Result<*const c_void, SurfaceCreationError<Self::Error>> {
        GbmSurface::recreate(self).map_err(SurfaceCreationError::NativeSurfaceCreationFailed)?;

        wrap_egl_call(|| {
            ffi::egl::CreateWindowSurface(
                display.handle,
                config_id,
                self.0.surface.borrow().as_raw() as *const _,
                surface_attributes.as_ptr(),
            )
        })
        .map_err(SurfaceCreationError::EGLSurfaceCreationFailed)
    }

    fn needs_recreation(&self) -> bool {
        self.needs_recreation()
    }

    fn swap_buffers(
        &self,
        display: &Arc<EGLDisplayHandle>,
        surface: ffi::egl::types::EGLSurface,
    ) -> Result<(), SwapBuffersError<Self::Error>> {
        wrap_egl_call(|| unsafe { ffi::egl::SwapBuffers(***display, surface as *const _) })
            .map_err(SwapBuffersError::EGLSwapBuffers)?;
        // this is safe since `eglSwapBuffers` will have been called exactly once
        // if this is used by our egl module, which is why this trait is unsafe.
        unsafe { self.page_flip() }.map_err(SwapBuffersError::Underlying)
    }
}
