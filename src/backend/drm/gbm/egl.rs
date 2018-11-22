use backend::drm::{connector, Device, RawDevice, RawSurface, Surface};
use backend::egl::error::Result as EglResult;
use backend::egl::ffi;
use backend::egl::native::{Backend, NativeDisplay, NativeSurface};
use backend::graphics::SwapBuffersError;

use super::error::{Error, Result};
use super::{GbmDevice, GbmSurface};

use drm::control::{crtc, Device as ControlDevice, Mode};
use gbm::AsRaw;
use std::iter::{FromIterator, IntoIterator};
use std::marker::PhantomData;
use std::ptr;

/// Gbm backend type
pub struct Gbm<D: RawDevice + 'static> {
    _userdata: PhantomData<D>,
}

impl<D: RawDevice + 'static> Backend for Gbm<D> {
    type Surface = GbmSurface<D>;

    unsafe fn get_display<F>(
        display: ffi::NativeDisplayType,
        has_dp_extension: F,
        log: ::slog::Logger,
    ) -> ffi::egl::types::EGLDisplay
    where
        F: Fn(&str) -> bool,
    {
        if has_dp_extension("EGL_KHR_platform_gbm") && ffi::egl::GetPlatformDisplay::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_KHR_platform_gbm");
            ffi::egl::GetPlatformDisplay(ffi::egl::PLATFORM_GBM_KHR, display as *mut _, ptr::null())
        } else if has_dp_extension("EGL_MESA_platform_gbm") && ffi::egl::GetPlatformDisplayEXT::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_MESA_platform_gbm");
            ffi::egl::GetPlatformDisplayEXT(ffi::egl::PLATFORM_GBM_MESA, display as *mut _, ptr::null())
        } else if has_dp_extension("EGL_MESA_platform_gbm") && ffi::egl::GetPlatformDisplay::is_loaded() {
            trace!(log, "EGL Display Initialization via EGL_MESA_platform_gbm");
            ffi::egl::GetPlatformDisplay(ffi::egl::PLATFORM_GBM_MESA, display as *mut _, ptr::null())
        } else {
            trace!(log, "Default EGL Display Initialization via GetDisplay");
            ffi::egl::GetDisplay(display as *mut _)
        }
    }
}

/// Arguments necessary to construct a `GbmSurface`
pub struct SurfaceArguments {
    /// Crtc
    pub crtc: crtc::Handle,
    /// Mode
    pub mode: Mode,
    /// Connectors
    pub connectors: Vec<connector::Handle>,
}

impl From<(crtc::Handle, Mode, Vec<connector::Handle>)> for SurfaceArguments {
    fn from((crtc, mode, connectors): (crtc::Handle, Mode, Vec<connector::Handle>)) -> Self {
        SurfaceArguments {
            crtc,
            mode,
            connectors: Vec::from_iter(connectors),
        }
    }
}

unsafe impl<D: RawDevice + ControlDevice + 'static> NativeDisplay<Gbm<D>> for GbmDevice<D> {
    type Arguments = SurfaceArguments;
    type Error = Error;

    fn is_backend(&self) -> bool {
        true
    }

    fn ptr(&self) -> EglResult<ffi::NativeDisplayType> {
        Ok(self.dev.borrow().as_raw() as *const _)
    }

    fn create_surface(&mut self, args: SurfaceArguments) -> Result<GbmSurface<D>> {
        Device::create_surface(self, args.crtc, args.mode, args.connectors)
    }
}

unsafe impl<D: RawDevice + 'static> NativeSurface for GbmSurface<D> {
    fn ptr(&self) -> ffi::NativeWindowType {
        self.0.surface.borrow().as_raw() as *const _
    }

    fn swap_buffers<F>(&self, flip: F) -> ::std::result::Result<(), SwapBuffersError>
    where
        F: FnOnce() -> ::std::result::Result<(), SwapBuffersError>,
    {
        if self.0.crtc.commit_pending() || {
            let fb = self.0.front_buffer.take();
            let res = fb.is_none();
            self.0.front_buffer.set(fb);
            res
        } {
            self.recreate(flip).map_err(|_| SwapBuffersError::ContextLost)
        } else {
            self.page_flip(flip)
        }
    }
}
