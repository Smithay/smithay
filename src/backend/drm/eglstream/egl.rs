//!
//! Egl [`NativeDisplay`](::backend::egl::native::NativeDisplay) and
//! [`NativeSurface`](::backend::egl::native::NativeSurface) support for
//! [`EglStreamDevice`](EglStreamDevice) and [`EglStreamSurface`](EglStreamSurface).
//!

#[cfg(feature = "backend_drm_atomic")]
use crate::backend::drm::atomic::AtomicDrmDevice;
#[cfg(all(feature = "backend_drm_atomic", feature = "backend_drm_legacy"))]
use crate::backend::drm::common::fallback::{EitherError, FallbackDevice, FallbackSurface};
#[cfg(any(feature = "backend_drm_atomic", feature = "backend_drm_legacy"))]
use crate::backend::drm::common::Error as DrmError;
#[cfg(feature = "backend_drm_legacy")]
use crate::backend::drm::legacy::LegacyDrmDevice;
use crate::backend::drm::{Device, RawDevice, RawSurface, Surface};
use crate::backend::egl::native::{Backend, NativeDisplay, NativeSurface};
use crate::backend::egl::{
    display::EGLDisplayHandle, ffi, wrap_egl_call, EGLError, Error as EglBackendError, SurfaceCreationError,
    SwapBuffersError,
};

use super::Error;
use super::{EglStreamDevice, EglStreamSurface};

use drm::control::{connector, crtc, Device as ControlDevice, Mode};
use nix::libc::{c_int, c_void};
use std::marker::PhantomData;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;

/// Egl Device backend type
///
/// See [`Backend`](::backend::egl::native::Backend).
pub struct EglStreamDeviceBackend<D: RawDevice + 'static> {
    _userdata: PhantomData<D>,
}

impl<D: RawDevice + 'static> Backend for EglStreamDeviceBackend<D>
where
    EglStreamSurface<D>: NativeSurface<Error = Error<<<D as Device>::Surface as Surface>::Error>>,
{
    type Surface = EglStreamSurface<D>;
    type Error = Error<<<D as Device>::Surface as Surface>::Error>;

    // create an EGLDisplay for the EGLstream platform
    unsafe fn get_display<F>(
        display: ffi::NativeDisplayType,
        attribs: &[ffi::EGLint],
        has_dp_extension: F,
        log: ::slog::Logger,
    ) -> Result<ffi::egl::types::EGLDisplay, EGLError>
    where
        F: Fn(&str) -> bool,
    {
        if has_dp_extension("EGL_EXT_platform_device") && ffi::egl::GetPlatformDisplayEXT::is_loaded() {
            debug!(
                log,
                "EGL Display Initialization via EGL_EXT_platform_device with {:?}", display
            );
            wrap_egl_call(|| {
                ffi::egl::GetPlatformDisplayEXT(
                    ffi::egl::PLATFORM_DEVICE_EXT,
                    display as *mut _,
                    attribs.as_ptr() as *const _,
                )
            })
        } else {
            Ok(ffi::egl::NO_DISPLAY)
        }
    }
}

unsafe impl<D: RawDevice + ControlDevice + 'static> NativeDisplay<EglStreamDeviceBackend<D>>
    for EglStreamDevice<D>
where
    EglStreamSurface<D>: NativeSurface<Error = Error<<<D as Device>::Surface as Surface>::Error>>,
{
    type Arguments = (crtc::Handle, Mode, Vec<connector::Handle>);

    fn is_backend(&self) -> bool {
        true
    }

    fn ptr(&self) -> Result<ffi::NativeDisplayType, EglBackendError> {
        Ok(self.dev as *const _)
    }

    fn attributes(&self) -> Vec<ffi::EGLint> {
        vec![
            ffi::egl::DRM_MASTER_FD_EXT as ffi::EGLint,
            self.raw.as_raw_fd(),
            ffi::egl::NONE as i32,
        ]
    }

    fn surface_type(&self) -> ffi::EGLint {
        ffi::egl::STREAM_BIT_KHR as ffi::EGLint
    }

    fn create_surface(
        &mut self,
        args: Self::Arguments,
    ) -> Result<EglStreamSurface<D>, Error<<<D as Device>::Surface as Surface>::Error>> {
        Device::create_surface(self, args.0, args.1, &args.2)
    }
}

// we need either a `crtc` or a `plane` for EGLStream initializations,
// which totally breaks our abstraction.. (This is normally an implementation details of `RawSurface`-implementations).
//
// as a result, we need three implemenations for atomic, legacy and fallback...

#[cfg(feature = "backend_drm_atomic")]
unsafe impl<A: AsRawFd + 'static> NativeSurface for EglStreamSurface<AtomicDrmDevice<A>> {
    type Error = Error<DrmError>;

    unsafe fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
        surface_attribs: &[c_int],
    ) -> Result<*const c_void, SurfaceCreationError<Self::Error>> {
        let output_attributes = {
            let mut out: Vec<isize> = Vec::with_capacity(3);
            out.push(ffi::egl::DRM_PLANE_EXT as isize);
            out.push(Into::<u32>::into(self.0.crtc.0.planes.primary) as isize);
            out.push(ffi::egl::NONE as isize);
            out
        };

        self.create_surface(display, config_id, surface_attribs, &output_attributes)
            .map_err(SurfaceCreationError::NativeSurfaceCreationFailed)
    }

    fn needs_recreation(&self) -> bool {
        self.0.crtc.commit_pending()
    }

    fn swap_buffers(
        &self,
        display: &Arc<EGLDisplayHandle>,
        surface: ffi::egl::types::EGLSurface,
    ) -> Result<(), SwapBuffersError<Error<DrmError>>> {
        if let Some((buffer, fb)) = self.0.commit_buffer.take() {
            let _ = self.0.crtc.destroy_framebuffer(fb);
            let _ = self.0.crtc.destroy_dumb_buffer(buffer);
        }

        self.flip(self.0.crtc.0.crtc, display, surface)
    }
}

#[cfg(feature = "backend_drm_legacy")]
unsafe impl<A: AsRawFd + 'static> NativeSurface for EglStreamSurface<LegacyDrmDevice<A>> {
    type Error = Error<DrmError>;

    unsafe fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
        surface_attribs: &[c_int],
    ) -> Result<*const c_void, SurfaceCreationError<Self::Error>> {
        let output_attributes = {
            let mut out: Vec<isize> = Vec::with_capacity(3);
            out.push(ffi::egl::DRM_CRTC_EXT as isize);
            out.push(Into::<u32>::into(self.0.crtc.0.crtc) as isize);
            out.push(ffi::egl::NONE as isize);
            out
        };

        self.create_surface(display, config_id, surface_attribs, &output_attributes)
            .map_err(SurfaceCreationError::NativeSurfaceCreationFailed)
    }

    fn needs_recreation(&self) -> bool {
        self.0.crtc.commit_pending()
    }

    fn swap_buffers(
        &self,
        display: &Arc<EGLDisplayHandle>,
        surface: ffi::egl::types::EGLSurface,
    ) -> Result<(), SwapBuffersError<Error<DrmError>>> {
        if let Some((buffer, fb)) = self.0.commit_buffer.take() {
            let _ = self.0.crtc.destroy_framebuffer(fb);
            let _ = self.0.crtc.destroy_dumb_buffer(buffer);
        }
        self.flip(self.0.crtc.0.crtc, display, surface)
    }
}

#[cfg(all(feature = "backend_drm_atomic", feature = "backend_drm_legacy"))]
unsafe impl<A: AsRawFd + 'static> NativeSurface
    for EglStreamSurface<FallbackDevice<AtomicDrmDevice<A>, LegacyDrmDevice<A>>>
{
    type Error = Error<EitherError<DrmError, DrmError>>;

    unsafe fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
        surface_attribs: &[c_int],
    ) -> Result<*const c_void, SurfaceCreationError<Self::Error>> {
        let output_attributes = {
            let mut out: Vec<isize> = Vec::with_capacity(3);
            match &self.0.crtc {
                FallbackSurface::Preference(dev) => {
                    out.push(ffi::egl::DRM_PLANE_EXT as isize);
                    out.push(Into::<u32>::into(dev.0.planes.primary) as isize);
                } //AtomicDrmSurface
                FallbackSurface::Fallback(dev) => {
                    out.push(ffi::egl::DRM_CRTC_EXT as isize);
                    out.push(Into::<u32>::into(dev.0.crtc) as isize);
                } // LegacyDrmSurface
            }
            out.push(ffi::egl::NONE as isize);
            out
        };

        self.create_surface(display, config_id, surface_attribs, &output_attributes)
            .map_err(SurfaceCreationError::NativeSurfaceCreationFailed)
    }

    fn needs_recreation(&self) -> bool {
        self.0.crtc.commit_pending()
    }

    fn swap_buffers(
        &self,
        display: &Arc<EGLDisplayHandle>,
        surface: ffi::egl::types::EGLSurface,
    ) -> Result<(), SwapBuffersError<Self::Error>> {
        if let Some((buffer, fb)) = self.0.commit_buffer.take() {
            let _ = self.0.crtc.destroy_framebuffer(fb);
            let _ = self.0.crtc.destroy_dumb_buffer(buffer);
        }
        let crtc = match &self.0.crtc {
            FallbackSurface::Preference(dev) => dev.0.crtc,
            FallbackSurface::Fallback(dev) => dev.0.crtc,
        };

        self.flip(crtc, display, surface)
    }
}
