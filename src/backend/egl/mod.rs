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

#[cfg(feature = "renderer_gl")]
use crate::backend::graphics::gl::{ffi as gl_ffi, GLGraphicsBackend};
use nix::libc::c_uint;
use std::fmt;
#[cfg(feature = "wayland_frontend")]
use wayland_server::{protocol::wl_buffer::WlBuffer, Display};

pub mod context;
pub use self::context::EGLContext;
mod error;
pub use self::error::Error;

use nix::libc::c_void;

#[allow(non_camel_case_types, dead_code, unused_mut, non_upper_case_globals)]
pub mod ffi;
use self::ffi::egl::types::EGLImage;

pub mod display;
pub mod native;
pub mod surface;
pub use self::surface::EGLSurface;
#[cfg(feature = "use_system_lib")]
use crate::backend::egl::display::EGLBufferReader;
use crate::backend::egl::display::EGLDisplayHandle;
#[cfg(feature = "renderer_gl")]
use std::ffi::CStr;
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
pub fn get_proc_address(symbol: &str) -> *const c_void {
    unsafe {
        let addr = CString::new(symbol.as_bytes()).unwrap();
        let addr = addr.as_ptr();
        ffi::egl::GetProcAddress(addr) as *const _
    }
}

/// Error that can occur when accessing an EGL buffer
#[cfg(feature = "wayland_frontend")]
#[derive(thiserror::Error)]
pub enum BufferAccessError {
    /// The corresponding Context is not alive anymore
    #[error("The corresponding context was lost")]
    ContextLost,
    /// This buffer is not managed by the EGL buffer
    #[error("This buffer is not managed by EGL")]
    NotManaged(WlBuffer),
    /// Failed to create `EGLImages` from the buffer
    #[error("Failed to create EGLImages from the buffer")]
    EGLImageCreationFailed,
    /// The required EGL extension is not supported by the underlying EGL implementation
    #[error("{0}")]
    EglExtensionNotSupported(#[from] EglExtensionNotSupportedError),
}

#[cfg(feature = "wayland_frontend")]
impl fmt::Debug for BufferAccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> ::std::result::Result<(), fmt::Error> {
        match *self {
            BufferAccessError::ContextLost => write!(formatter, "BufferAccessError::ContextLost"),
            BufferAccessError::NotManaged(_) => write!(formatter, "BufferAccessError::NotManaged"),
            BufferAccessError::EGLImageCreationFailed => {
                write!(formatter, "BufferAccessError::EGLImageCreationFailed")
            }
            BufferAccessError::EglExtensionNotSupported(ref err) => write!(formatter, "{:?}", err),
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
#[derive(Debug)]
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
pub struct EGLImages {
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
    buffer: WlBuffer,
    #[cfg(feature = "renderer_gl")]
    gl: gl_ffi::Gles2,
}

#[cfg(feature = "wayland_frontend")]
impl EGLImages {
    /// Amount of planes of these `EGLImages`
    pub fn num_planes(&self) -> usize {
        self.format.num_planes()
    }

    /// Bind plane to an OpenGL texture id
    ///
    /// This does only temporarily modify the OpenGL state any changes are reverted before returning.
    /// The given `GLGraphicsBackend` must be the one belonging to the `tex_id` and will be the current
    /// context (and surface if applicable) after this function returns.
    ///
    /// # Safety
    ///
    /// The given `tex_id` needs to be a valid GL texture in the given context otherwise undefined behavior might occur.
    #[cfg(feature = "renderer_gl")]
    pub unsafe fn bind_to_texture(
        &self,
        plane: usize,
        tex_id: c_uint,
        backend: &dyn GLGraphicsBackend,
    ) -> ::std::result::Result<(), TextureCreationError> {
        // receive the list of extensions for *this* context
        backend
            .make_current()
            .map_err(|_| TextureCreationError::ContextLost)?;

        let egl_to_texture_support = {
            // the list of gl extensions supported by the context
            let data = CStr::from_ptr(self.gl.GetString(gl_ffi::EXTENSIONS) as *const _)
                .to_bytes()
                .to_vec();
            let list = String::from_utf8(data).unwrap();
            list.split(' ')
                .any(|s| s == "GL_OES_EGL_image" || s == "GL_OES_EGL_image_base")
        };
        if !egl_to_texture_support {
            return Err(TextureCreationError::GLExtensionNotSupported("GL_OES_EGL_image"));
        }

        let mut old_tex_id: i32 = 0;
        self.gl.GetIntegerv(gl_ffi::TEXTURE_BINDING_2D, &mut old_tex_id);
        self.gl.BindTexture(gl_ffi::TEXTURE_2D, tex_id);
        self.gl.EGLImageTargetTexture2DOES(
            gl_ffi::TEXTURE_2D,
            *self
                .images
                .get(plane)
                .ok_or(TextureCreationError::PlaneIndexOutOfBounds)?,
        );
        let res = match ffi::egl::GetError() as u32 {
            ffi::egl::SUCCESS => Ok(()),
            err => Err(TextureCreationError::TextureBindingFailed(err)),
        };
        self.gl.BindTexture(gl_ffi::TEXTURE_2D, old_tex_id as u32);
        res
    }
}

#[cfg(feature = "wayland_frontend")]
impl Drop for EGLImages {
    fn drop(&mut self) {
        for image in self.images.drain(..) {
            unsafe {
                ffi::egl::DestroyImageKHR(**self.display, image);
            }
        }
        self.buffer.release();
    }
}

/// Trait any backend type may implement that allows binding a [`Display`](wayland_server::Display)
/// to create an [`EGLBufferReader`](display::EGLBufferReader) for EGL-based [`WlBuffer`]s.
#[cfg(feature = "use_system_lib")]
pub trait EGLGraphicsBackend {
    /// Binds this EGL context to the given Wayland display.
    ///
    /// This will allow clients to utilize EGL to create hardware-accelerated
    /// surfaces. The server will need to be able to handle EGL-[`WlBuffer`]s.
    ///
    /// ## Errors
    ///
    /// This might return [`EglExtensionNotSupported`](ErrorKind::EglExtensionNotSupported)
    /// if binding is not supported by the EGL implementation.
    ///
    /// This might return [`OtherEGLDisplayAlreadyBound`](ErrorKind::OtherEGLDisplayAlreadyBound)
    /// if called for the same [`Display`] multiple times, as only one context may be bound at any given time.
    fn bind_wl_display(&self, display: &Display) -> Result<EGLBufferReader, Error>;
}
