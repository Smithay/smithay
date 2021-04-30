//! Common traits and types for egl rendering
//!
//! Large parts of this module are taken from
//! [glutin src/api/egl](https://github.com/tomaka/glutin/tree/044e651edf67a2029eecc650dd42546af1501414/src/api/egl/)
//!
//! It therefore falls under
//! [glutin's Apache 2.0 license](https://github.com/tomaka/glutin/tree/044e651edf67a2029eecc650dd42546af1501414/LICENSE)
//!
//! Wayland specific EGL functionality - EGL based [`WlBuffer`](wayland_server::protocol::wl_buffer::WlBuffer)s.
//!
//! The types of this module can be used to initialize hardware acceleration rendering
//! based on EGL for clients as it may enabled usage of `EGLImage` based [`WlBuffer`](wayland_server::protocol::wl_buffer::WlBuffer)s.
//!
//! To use it bind any backend implementing the [`EGLGraphicsBackend`](::backend::egl::EGLGraphicsBackend) trait, that shall do the
//! rendering (so pick a fast one), to the [`wayland_server::Display`] of your compositor.
//! Note only one backend may be bound to any [`Display`](wayland_server::Display) at any time.
//!
//! You may then use the resulting [`EGLDisplay`](::backend::egl::EGLDisplay) to receive [`EGLImages`](::backend::egl::EGLImages)
//! of an EGL-based [`WlBuffer`](wayland_server::protocol::wl_buffer::WlBuffer) for rendering.

/*
#[cfg(feature = "renderer_gl")]
use crate::backend::graphics::{
    gl::{ffi as gl_ffi, GLGraphicsBackend},
    SwapBuffersError as GraphicsSwapBuffersError,
};
*/
use std::fmt;

pub mod context;
pub use self::context::EGLContext;
mod error;
pub use self::error::*;
use crate::backend::SwapBuffersError as GraphicsSwapBuffersError;

use nix::libc::c_void;

#[allow(non_camel_case_types, dead_code, unused_mut, non_upper_case_globals)]
pub mod ffi;
use self::ffi::egl::types::EGLImage;

pub mod display;
pub mod native;
pub mod surface;
pub use self::display::EGLDisplay;
pub use self::surface::EGLSurface;

use self::display::EGLDisplayHandle;
use std::ffi::CString;
use std::sync::Arc;

/// Error that can happen on optional EGL features
#[derive(Debug, Clone, PartialEq)]
pub struct EglExtensionNotSupportedError(&'static [&'static str]);

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
    /// Failed to create `EGLImages` from the buffer
    #[error("Failed to create EGLImages from the buffer. Err: {0:}")]
    EGLImageCreationFailed(#[source] EGLError),
    /// The required EGL extension is not supported by the underlying EGL implementation
    #[error("{0}")]
    EglExtensionNotSupported(#[from] EglExtensionNotSupportedError),
    /// We currently do not support multi-planar buffers
    #[error("Multi-planar formats (like {0:?}) are unsupported")]
    UnsupportedMultiPlanarFormat(Format),
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

/// Error that might happen when binding an `EGLImage` to a GL texture
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TextureCreationError {
    /// The given plane index is out of bounds
    #[error("This buffer is not managed by EGL")]
    PlaneIndexOutOfBounds,
    /// The OpenGL context has been lost and needs to be recreated.
    ///
    /// All the objects associated to it (textures, buffers, programs, etc.)
    /// need to be recreated from scratch.
    ///
    /// Operations will have no effect. Functions that read textures, buffers, etc.
    /// from OpenGL will return uninitialized data instead.
    ///
    /// A context loss usually happens on mobile devices when the user puts the
    /// application on sleep and wakes it up later. However any OpenGL implementation
    /// can theoretically lose the context at any time.
    #[error("The context has been lost, it needs to be recreated")]
    ContextLost,
    /// Required OpenGL Extension for texture creation is missing
    #[error("Required OpenGL Extension for texture creation is missing: {0}")]
    GLExtensionNotSupported(&'static str),
    /// Failed to bind the `EGLImage` to the given texture
    ///
    /// The given argument is the GL error code
    #[error("Failed to create EGLImages from the buffer (GL error code {0:x}")]
    TextureBindingFailed(u32),
}

/// Texture format types
#[repr(i32)]
#[allow(non_camel_case_types)]
#[derive(Debug, PartialEq, Eq)]
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

/// Images of the EGL-based [`WlBuffer`].
#[cfg(feature = "wayland_frontend")]
#[derive(Debug)]
pub struct EGLBuffer {
    display: Arc<EGLDisplayHandle>,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// If the y-axis is inverted or not
    pub y_inverted: bool,
    /// Format of these images
    pub format: Format,
    images: Vec<EGLImage>,
}

#[cfg(feature = "wayland_frontend")]
impl EGLBuffer {
    /// Amount of planes of these `EGLImages`
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
