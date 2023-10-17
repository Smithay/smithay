//! EGL fence related structs

use std::os::unix::io::{FromRawFd, IntoRawFd, OwnedFd};
use std::sync::Arc;
use std::time::Duration;

use crate::backend::egl::{
    ffi, wrap_egl_call, wrap_egl_call_bool, wrap_egl_call_ptr, EGLDisplay, EGLDisplayHandle, Error,
};

#[derive(Debug)]
struct InnerEGLFence {
    display_handle: Arc<EGLDisplayHandle>,
    handle: ffi::egl::types::EGLSync,
    native: bool,
}
unsafe impl Send for InnerEGLFence {}
unsafe impl Sync for InnerEGLFence {}

impl Drop for InnerEGLFence {
    fn drop(&mut self) {
        unsafe {
            // Not much we can do about when this fails
            ffi::egl::DestroySync(**self.display_handle, self.handle);
        }
    }
}

/// A EGL fence
#[derive(Debug, Clone)]
pub struct EGLFence(Arc<InnerEGLFence>);

impl EGLFence {
    /// Returns whether the display supports creating fences from native fences
    pub fn supports_importing(display: &EGLDisplay) -> bool {
        display.supports_native_fences
    }

    /// Import a native fence fd
    #[profiling::function]
    pub fn import(display: &EGLDisplay, native: OwnedFd) -> Result<Self, Error> {
        // SAFETY: we do not have to test for EGL_KHR_fence_sync as EGL_ANDROID_native_fence_sync
        // requires it already
        if !display.supports_native_fences {
            return Err(Error::EglExtensionNotSupported(&[
                "EGL_ANDROID_native_fence_sync",
            ]));
        }

        let display_handle = display.get_display_handle();
        let fd = native.into_raw_fd();
        let attributes: [ffi::egl::types::EGLAttrib; 3] = [
            ffi::egl::SYNC_NATIVE_FENCE_FD_ANDROID as ffi::egl::types::EGLAttrib,
            fd as ffi::egl::types::EGLAttrib,
            ffi::egl::NONE as ffi::egl::types::EGLAttrib,
        ];

        let res = wrap_egl_call_ptr(|| unsafe {
            ffi::egl::CreateSync(
                **display_handle,
                ffi::egl::SYNC_NATIVE_FENCE_ANDROID,
                attributes.as_ptr(),
            )
        })
        .map_err(Error::CreationFailed);

        // In case of an error we have to close the fd to not leak it
        if res.is_err() {
            unsafe {
                rustix::io::close(fd);
            }
        }

        let handle = res?;

        Ok(Self(Arc::new(InnerEGLFence {
            display_handle,
            handle,
            native: true,
        })))
    }

    /// Create a new egl fence
    ///
    /// This will create a `EGL_SYNC_NATIVE_FENCE_ANDROID` or a `EGL_SYNC_FENCE_KHR` depending
    /// on the availability of the `EGL_ANDROID_native_fence_sync` extension.
    ///
    /// Returns `Err` if the `EGL_KHR_fence_sync` is not present or creating the fence failed.
    ///
    /// Note: It is the callers responsibility to make sure the current bound client API supports
    /// fences. For OpenglES the `GL_OES_EGL_sync` extension indicates support for fences.
    #[profiling::function]
    pub fn create(display: &EGLDisplay) -> Result<Self, Error> {
        if !display.has_fences {
            return Err(Error::EglExtensionNotSupported(&["EGL_KHR_fence_sync"]));
        }

        let (type_, native) = if display.supports_native_fences {
            (ffi::egl::SYNC_NATIVE_FENCE_ANDROID, true)
        } else {
            (ffi::egl::SYNC_FENCE, false)
        };

        let display_handle = display.get_display_handle();
        let handle =
            wrap_egl_call_ptr(|| unsafe { ffi::egl::CreateSync(**display_handle, type_, std::ptr::null()) })
                .map_err(Error::CreationFailed)?;

        Ok(Self(Arc::new(InnerEGLFence {
            display_handle,
            handle,
            native,
        })))
    }

    /// Indicates if this fence represents a native fence
    pub fn is_native(&self) -> bool {
        self.0.native
    }

    /// Export this [`EGLFence`] as a native fence fd
    #[profiling::function]
    pub fn export(&self) -> Result<OwnedFd, Error> {
        if !self.0.native {
            return Err(Error::EglExtensionNotSupported(&[
                "EGL_ANDROID_native_fence_sync",
            ]));
        }

        let fd = wrap_egl_call(
            || unsafe { ffi::egl::DupNativeFenceFDANDROID(**self.0.display_handle, self.0.handle) },
            ffi::egl::NO_NATIVE_FENCE_FD_ANDROID,
        )
        .map_err(Error::CreationFailed)?;
        // SAFETY: eglDupNativeFenceFDANDROID generates EGL_BAD_PARAMETER in case of an error, so fd should always
        // contain a valid fd
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Tries to insert the fence in the current bound client context
    ///
    /// Returns `Err` if the supplied display does not match the display
    /// used at creation time of the fence.
    #[profiling::function]
    pub fn wait(&self, display: &EGLDisplay) -> Result<(), Error> {
        if **display.get_display_handle() != **self.0.display_handle {
            return Err(Error::DisplayNotSupported);
        }

        wrap_egl_call_bool(|| unsafe { ffi::egl::WaitSync(**self.0.display_handle, self.0.handle, 0) })
            .map_err(Error::CreationFailed)?;
        Ok(())
    }

    /// Blocks the current thread until the fence is signaled or the supplied
    /// timeout is reached.
    ///
    /// If the timeout is reached `false` is returned
    #[profiling::function]
    pub fn client_wait(&self, timeout: Option<Duration>, flush: bool) -> bool {
        let timeout = timeout
            .map(|t| t.as_nanos() as ffi::egl::types::EGLuint64KHR)
            .unwrap_or(ffi::egl::FOREVER);
        let flags = if flush {
            ffi::egl::SYNC_FLUSH_COMMANDS_BIT as ffi::egl::types::EGLint
        } else {
            0
        };
        // SAFETY: eglClientWaitSyncKHR only defines two errors
        // 1. If <sync> is not a valid sync object for <dpy>, EGL_FALSE is returned and an EGL_BAD_PARAMETER error is generated.
        // 2. If <dpy> does not match the EGLDisplay passed to eglCreateSyncKHR when <sync> was created, the behaviour is undefined.
        // Both should not be possible as we own both, the handle and the display the handle was created on
        let status = wrap_egl_call(
            || unsafe { ffi::egl::ClientWaitSync(**self.0.display_handle, self.0.handle, flags, timeout) },
            ffi::egl::FALSE as ffi::egl::types::EGLint,
        )
        .unwrap();

        status == ffi::egl::CONDITION_SATISFIED as ffi::EGLint
    }
}
