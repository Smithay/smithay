//! Common traits and types for egl rendering
//!
//! This module has multiple responsibilities related to functionality provided by libEGL:
//! Initializing EGL objects to:
//! - initialize usage of EGL based [`WlBuffer`](wayland_server::protocol::wl_buffer::WlBuffer)s via `wl_drm`.
//! - initialize OpenGL contexts from.
//! - Import/Export external resources to/from OpenGL
//!
//! To use this module, you first need to create an [`EGLDisplay`] through a supported EGL platform
//! as indicated by an implementation of the [`native::EGLNativeDisplay`] trait.
//!
//! You may bind the [`EGLDisplay`], that shall be used by clients for rendering (so pick one initialized by a fast platform)
//! to the [`wayland_server::Display`] of your compositor. Note only one backend may be bound to any [`Display`](wayland_server::Display) at any time.
//!
//! You may then use the resulting [`display::EGLBufferReader`] to receive [`EGLBuffer`]
//! of an EGL-based [`WlBuffer`](wayland_server::protocol::wl_buffer::WlBuffer) for rendering.
//! Renderers implementing the [`ImportEgl`](crate::backend::renderer::ImportEgl)-trait can manage the buffer reader for you.
//!
//! **Note:** The support for binding the [`EGLDisplay`] for use by clients requires the `use_system_lib` cargo feature on Smithay.
//!
//! To create OpenGL contexts you may create [`EGLContext`]s from the display and if the context is initialized with a config
//! it may also be used to initialize an [`EGLSurface`], which can be [bound](crate::backend::renderer::Bind) to some renderers.
//!
//! Alternatively you may import [`dmabuf`](crate::backend::allocator::dmabuf)s using the display, which result
//! in an [`EGLImage`], which can be rendered into by OpenGL. This is preferable to using surfaces as the dmabuf can be
//! passed around freely making resource-management and more complex use-cases like Multi-GPU rendering easier to manage.
//! Renderers based on EGL may support doing this for you by allowing you to [`Bind`](crate::backend::renderer::Bind) a dmabuf directly.
//!

use std::ffi::c_void;
use std::fmt;

pub mod context;
pub use self::context::EGLContext;
mod device;
mod error;
pub use self::error::*;
use crate::backend::SwapBuffersError as GraphicsSwapBuffersError;
#[cfg(feature = "wayland_frontend")]
use crate::utils::{Buffer, Size};

#[allow(non_camel_case_types, dead_code, unused_mut, non_upper_case_globals)]
pub mod ffi;
use self::display::EGLDisplayHandle;
#[cfg(feature = "wayland_frontend")]
use self::ffi::egl::types::EGLImage;

pub mod display;
pub mod fence;
pub mod native;
pub mod surface;
pub use self::device::EGLDevice;
pub use self::display::EGLDisplay;
pub use self::surface::EGLSurface;

use std::ffi::CString;
#[cfg(feature = "wayland_frontend")]
use std::sync::Arc;

/// Error that can happen on optional EGL features
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EglExtensionNotSupportedError(pub &'static [&'static str]);

impl fmt::Display for EglExtensionNotSupportedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> ::std::result::Result<(), fmt::Error> {
        write!(
            formatter,
            "None of the following EGL extensions is supported by the underlying EGL implementation,
                     at least one is required: {:?}",
            self.0
        )
    }
}

impl ::std::error::Error for EglExtensionNotSupportedError {}

/// Returns the address of an OpenGL function.
///
/// Result is independent of displays and does not guarantee an extension is actually supported at runtime.
///
/// # Safety
///
/// This function should only be invoked while an EGL context is active.
pub unsafe fn get_proc_address(symbol: &str) -> *const c_void {
    let addr = CString::new(symbol.as_bytes()).unwrap();
    let addr = addr.as_ptr();
    ffi::egl::GetProcAddress(addr) as *const _
}

/// Error that can occur when accessing an EGL buffer
#[cfg(feature = "wayland_frontend")]
#[derive(thiserror::Error)]
pub enum BufferAccessError {
    /// The corresponding Context is not alive anymore
    #[error("The corresponding context was lost")]
    ContextLost,
    /// This buffer is not managed by the EGL buffer
    #[error("This buffer is not managed by EGL. Err: {0:}")]
    NotManaged(#[source] EGLError),
    /// Failed to create `EGLBuffer` from the buffer
    #[error("Failed to create EGLImages from the buffer. Err: {0:}")]
    EGLImageCreationFailed(#[source] EGLError),
    /// The required EGL extension is not supported by the underlying EGL implementation
    #[error("{0}")]
    EglExtensionNotSupported(#[from] EglExtensionNotSupportedError),
    /// We currently do not support multi-planar buffers
    #[error("Multi-planar formats (like {0:?}) are unsupported")]
    UnsupportedMultiPlanarFormat(Format),
    /// This buffer has been destroyed
    #[error("This buffer has been destroyed")]
    Destroyed,
}

#[cfg(feature = "wayland_frontend")]
impl fmt::Debug for BufferAccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> ::std::result::Result<(), fmt::Error> {
        match *self {
            BufferAccessError::ContextLost => write!(formatter, "BufferAccessError::ContextLost"),
            BufferAccessError::NotManaged(_) => write!(formatter, "BufferAccessError::NotManaged"),
            BufferAccessError::EGLImageCreationFailed(_) => {
                write!(formatter, "BufferAccessError::EGLImageCreationFailed")
            }
            BufferAccessError::EglExtensionNotSupported(ref err) => write!(formatter, "{:?}", err),
            BufferAccessError::UnsupportedMultiPlanarFormat(ref fmt) => write!(
                formatter,
                "BufferAccessError::UnsupportedMultiPlanerFormat({:?})",
                fmt
            ),
            BufferAccessError::Destroyed => write!(formatter, "BufferAccessError::Destroyed"),
        }
    }
}

/// Error that can happen when swapping buffers.
#[derive(Debug, thiserror::Error)]
pub enum SwapBuffersError {
    /// EGL error during `eglSwapBuffers`
    #[error("{0:}")]
    EGLSwapBuffers(#[source] EGLError),
    /// EGL error during surface creation
    #[error("{0:}")]
    EGLCreateSurface(#[source] EGLError),
}

impl std::convert::From<SwapBuffersError> for GraphicsSwapBuffersError {
    fn from(value: SwapBuffersError) -> Self {
        match value {
            // bad surface is answered with a surface recreation in `swap_buffers`
            x @ SwapBuffersError::EGLSwapBuffers(EGLError::BadSurface) => {
                GraphicsSwapBuffersError::TemporaryFailure(Box::new(x))
            }
            // the rest is either never happening or are unrecoverable
            x @ SwapBuffersError::EGLSwapBuffers(_) => GraphicsSwapBuffersError::ContextLost(Box::new(x)),
            x @ SwapBuffersError::EGLCreateSurface(_) => GraphicsSwapBuffersError::ContextLost(Box::new(x)),
        }
    }
}

/// Error that can happen when making a context (and surface) current on the active thread.
#[derive(thiserror::Error, Debug)]
#[error("`eglMakeCurrent` failed: {0}")]
pub struct MakeCurrentError(#[from] EGLError);

impl From<MakeCurrentError> for GraphicsSwapBuffersError {
    fn from(err: MakeCurrentError) -> GraphicsSwapBuffersError {
        match err {
            /*
            From khronos docs:
                If draw or read are not compatible with context, then an EGL_BAD_MATCH error is generated.
                If context is current to some other thread, or if either draw or read are bound to contexts in another thread, an EGL_BAD_ACCESS error is generated.
                If binding context would exceed the number of current contexts of that client API type supported by the implementation, an EGL_BAD_ACCESS error is generated.
                If either draw or read are pbuffers created with eglCreatePbufferFromClientBuffer, and the underlying bound client API buffers are in use by the client API that created them, an EGL_BAD_ACCESS error is generated.

            Except for the first case all of these recoverable. This conversation is mostly used in winit & EglSurface, where compatible context and surfaces are build.
            */
            x @ MakeCurrentError(EGLError::BadAccess) => {
                GraphicsSwapBuffersError::TemporaryFailure(Box::new(x))
            }
            // BadSurface would result in a recreation in `eglSwapBuffers` -> recoverable
            x @ MakeCurrentError(EGLError::BadSurface) => {
                GraphicsSwapBuffersError::TemporaryFailure(Box::new(x))
            }
            /*
            From khronos docs:
                If the previous context of the calling thread has unflushed commands, and the previous surface is no longer valid, an EGL_BAD_CURRENT_SURFACE error is generated.

            This does not consern this or future `makeCurrent`-calls.
            */
            x @ MakeCurrentError(EGLError::BadCurrentSurface) => {
                GraphicsSwapBuffersError::TemporaryFailure(Box::new(x))
            }
            // the rest is either never happening or are unrecoverable
            x => GraphicsSwapBuffersError::ContextLost(Box::new(x)),
        }
    }
}

/// Texture format types
#[repr(i32)]
#[allow(non_camel_case_types)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Format {
    /// RGB format
    RGB = ffi::egl::TEXTURE_RGB as i32,
    /// RGB + alpha channel format
    RGBA = ffi::egl::TEXTURE_RGBA as i32,
    /// External format
    External = ffi::egl::TEXTURE_EXTERNAL_WL,
    /// 2-plane Y and UV format
    Y_UV = ffi::egl::TEXTURE_Y_UV_WL,
    /// 3-plane Y, U and V format
    Y_U_V = ffi::egl::TEXTURE_Y_U_V_WL,
    /// 2-plane Y and XUXV format
    Y_XUXV = ffi::egl::TEXTURE_Y_XUXV_WL,
}

impl Format {
    /// Amount of planes this format uses
    pub fn num_planes(&self) -> usize {
        match *self {
            Format::RGB | Format::RGBA | Format::External => 1,
            Format::Y_UV | Format::Y_XUXV => 2,
            Format::Y_U_V => 3,
        }
    }
}

/// Images of the EGL-based [`WlBuffer`](wayland_server::protocol::wl_buffer::WlBuffer).
#[cfg(feature = "wayland_frontend")]
#[derive(Debug)]
pub struct EGLBuffer {
    display: Arc<EGLDisplayHandle>,
    /// Size of the buffer
    pub size: Size<i32, Buffer>,
    /// If the y-axis is inverted or not
    pub y_inverted: bool,
    /// Format of these images
    pub format: Format,
    images: Vec<EGLImage>,
}

#[cfg(feature = "wayland_frontend")]
impl EGLBuffer {
    /// Amount of planes of this EGLBuffer
    pub fn num_planes(&self) -> usize {
        self.format.num_planes()
    }

    /// Returns the `EGLImage` handle for a given plane
    pub fn image(&self, plane: usize) -> Option<EGLImage> {
        if plane > self.format.num_planes() {
            None
        } else {
            Some(self.images[plane])
        }
    }

    /// Returns the underlying images
    pub fn into_images(mut self) -> Vec<EGLImage> {
        self.images.drain(..).collect()
    }
}

#[cfg(feature = "wayland_frontend")]
impl Drop for EGLBuffer {
    fn drop(&mut self) {
        for image in self.images.drain(..) {
            // ignore result on drop
            unsafe {
                ffi::egl::DestroyImageKHR(**self.display, image);
            }
        }
    }
}
