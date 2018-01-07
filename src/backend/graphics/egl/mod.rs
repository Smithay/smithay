//! Common traits and types for egl context creation and rendering

/// Large parts of this module are taken from
/// https://github.com/tomaka/glutin/tree/044e651edf67a2029eecc650dd42546af1501414/src/api/egl/
///
/// It therefore falls under glutin's Apache 2.0 license
/// (see https://github.com/tomaka/glutin/tree/044e651edf67a2029eecc650dd42546af1501414/LICENSE)
use super::GraphicsBackend;
use nix::libc::c_void;
use std::fmt;

pub mod context;
pub use self::context::EGLContext;
pub mod error;
#[allow(non_camel_case_types, dead_code, unused_mut, non_upper_case_globals)]
pub mod ffi;
pub mod native;
pub mod surface;
pub use self::surface::EGLSurface;
pub mod wayland;
pub use self::wayland::{EGLWaylandExtensions, EGLImages, BufferAccessError};

/// Error that can happen when swapping buffers.
#[derive(Debug, Clone, PartialEq)]
pub enum SwapBuffersError {
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
    ContextLost,
    /// The buffers have already been swapped.
    ///
    /// This error can be returned when `swap_buffers` has been called multiple times
    /// without any modification in between.
    AlreadySwapped,
    /// Unknown GL error
    Unknown(u32),
}

impl fmt::Display for SwapBuffersError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> ::std::result::Result<(), fmt::Error> {
        use std::error::Error;
        write!(formatter, "{}", self.description())
    }
}

impl ::std::error::Error for SwapBuffersError {
    fn description(&self) -> &str {
        match *self {
            SwapBuffersError::ContextLost => "The context has been lost, it needs to be recreated",
            SwapBuffersError::AlreadySwapped => {
                "Buffers are already swapped, swap_buffers was called too many times"
            }
            SwapBuffersError::Unknown(_) => "Unknown Open GL error occurred",
        }
    }

    fn cause(&self) -> Option<&::std::error::Error> {
        None
    }
}

/// Error that can happen on optional EGL features
#[derive(Debug, Clone, PartialEq)]
pub struct EglExtensionNotSupportedError(&'static [&'static str]);

impl fmt::Display for EglExtensionNotSupportedError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> ::std::result::Result<(), fmt::Error> {
        write!(formatter, "None of the following EGL extensions is supported by the underlying EGL implementation,
                     at least one is required: {:?}", self.0)
    }
}

impl ::std::error::Error for EglExtensionNotSupportedError {
    fn description(&self) -> &str {
        "The required EGL extension is not supported by the underlying EGL implementation"
    }

    fn cause(&self) -> Option<&::std::error::Error> {
        None
    }
}

/// Describes the pixel format of the main framebuffer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelFormat {
    /// is the format hardware accelerated
    pub hardware_accelerated: bool,
    /// number of bits used for colors
    pub color_bits: u8,
    /// number of bits used for alpha channel
    pub alpha_bits: u8,
    /// number of bits used for depth channel
    pub depth_bits: u8,
    /// number of bits used for stencil buffer
    pub stencil_bits: u8,
    /// is stereoscopy enabled
    pub stereoscopy: bool,
    /// is double buffering enabled
    pub double_buffer: bool,
    /// number of samples used for multisampling if enabled
    pub multisampling: Option<u16>,
    /// is srgb enabled
    pub srgb: bool,
}

/// Trait that describes objects that have an OpenGL context
/// and can be used to render upon
pub trait EGLGraphicsBackend: GraphicsBackend {
    /// Swaps buffers at the end of a frame.
    fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError>;

    /// Returns the address of an OpenGL function.
    ///
    /// Supposes that the context has been made current before this function is called.
    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void;

    /// Returns the dimensions of the window, or screen, etc in points.
    ///
    /// That are the scaled pixels of the underlying graphics backend.
    /// For nested compositors this will respect the scaling of the root compositor.
    /// For drawing directly onto hardware this unit will be equal to actual pixels.
    fn get_framebuffer_dimensions(&self) -> (u32, u32);

    /// Returns true if the OpenGL context is the current one in the thread.
    fn is_current(&self) -> bool;

    /// Makes the OpenGL context the current context in the current thread.
    ///
    /// # Unsafety
    ///
    /// This function is marked unsafe, because the context cannot be made current
    /// on multiple threads.
    unsafe fn make_current(&self) -> ::std::result::Result<(), SwapBuffersError>;

    /// Returns the pixel format of the main framebuffer of the context.
    fn get_pixel_format(&self) -> PixelFormat;
}
