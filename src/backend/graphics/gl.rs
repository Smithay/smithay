use nix::libc::c_void;

use super::{SwapBuffersError, PixelFormat};

#[cfg_attr(feature = "cargo-clippy", allow(clippy))]
#[allow(missing_docs)]
pub(crate) mod ffi {
    include!(concat!(env!("OUT_DIR"), "/gl_bindings.rs"));
}

pub use self::ffi::Gles2;

/// Trait that describes objects that have an OpenGL context
/// and can be used to render upon
pub trait GLGraphicsBackend {
    /// Swaps buffers at the end of a frame.
    fn swap_buffers(&self) -> Result<(), SwapBuffersError>;

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
    unsafe fn make_current(&self) -> Result<(), SwapBuffersError>;

    /// Returns the pixel format of the main framebuffer of the context.
    fn get_pixel_format(&self) -> PixelFormat;
}

/// Loads a Raw GLES Interface for a given `GLGraphicsBackend`
///
/// This remains valid as long as the underlying `GLGraphicsBackend` is alive
/// and may only be used in combination with the backend. Using this with any
/// other gl context may cause undefined behavior.
pub fn load_raw_gl<B: GLGraphicsBackend>(backend: &B) -> Gles2 {
    Gles2::load_with(|s| unsafe { backend.get_proc_address(s) as *const _ })
}
