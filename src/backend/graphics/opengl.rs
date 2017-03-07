//! Common traits and types for opengl rendering on graphics backends

use nix::c_void;

/// Error that can happen when swapping buffers.
#[derive(Debug, Clone)]
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
    AlreadySwapped,
}

/// All APIs related to OpenGL that you can possibly get
/// through OpenglRenderer implementations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    /// The classical OpenGL. Available on Windows, Linux, OS/X.
    OpenGl,
    /// OpenGL embedded system. Available on Linux, Android.
    OpenGlEs,
    /// OpenGL for the web. Very similar to OpenGL ES.
    WebGl,
}

/// Describes the pixel format of the main framebuffer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelFormat {
    /// Is the format hardware accelerated
    pub hardware_accelerated: bool,
    /// bits used for colors
    pub color_bits: u8,
    /// bits used for alpha channel
    pub alpha_bits: u8,
    /// bits used for depth channel
    pub depth_bits: u8,
    /// bits used for stencil buffer
    pub stencil_bits: u8,
    /// is stereoscopy enabled
    pub stereoscopy: bool,
    /// is double buffering enabled
    pub double_buffer: bool,
    /// multisampling format used if enabled
    pub multisampling: Option<u16>,
    /// if the format has srgb enabled
    pub srgb: bool,
}

/// Trait that describes objects that have an OpenGl context
/// and can be used to render upon
pub trait OpenglRenderer
{
    /// Swaps buffers at the end of a frame.
    fn swap_buffers(&self) -> Result<(), SwapBuffersError>;

    /// Returns the address of an OpenGL function.
    ///
    /// Supposes that the context has been made current before this function is called.
    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void;

    /// Returns the dimensions of the window, or screen, etc.
    fn get_framebuffer_dimensions(&self) -> (u32, u32);

    /// Returns true if the OpenGL context is the current one in the thread.
    fn is_current(&self) -> bool;

    /// Makes the OpenGL context the current context in the current thread.
    unsafe fn make_current(&self);

    /// Returns the OpenGL API being used.
    fn get_api(&self) -> Api;

    /// Returns the pixel format of the main framebuffer of the context.
    fn get_pixel_format(&self) -> PixelFormat;
}
