use glium::backend::Backend;
use glium::SwapBuffersError as GliumSwapBuffersError;

use std::os::raw::c_void;

use ::backend::graphics::opengl::{OpenglGraphicsBackend, SwapBuffersError};

impl From<SwapBuffersError> for GliumSwapBuffersError
{
    fn from(error: SwapBuffersError) -> Self {
        match error {
            SwapBuffersError::ContextLost => GliumSwapBuffersError::ContextLost,
            SwapBuffersError::AlreadySwapped => GliumSwapBuffersError::AlreadySwapped,
        }
    }
}

pub struct GliumGraphicBackend<T: OpenglGraphicsBackend>(T);

pub trait IntoGlium: OpenglGraphicsBackend + Sized
{
    fn into_glium(self) -> GliumGraphicBackend<Self>;
}

impl<T: OpenglGraphicsBackend> IntoGlium for T
{
    fn into_glium(self) -> GliumGraphicBackend<Self>
    {
        GliumGraphicBackend(self)
    }
}

unsafe impl<T: OpenglGraphicsBackend> Backend for GliumGraphicBackend<T>
{
    fn swap_buffers(&self) -> Result<(), GliumSwapBuffersError> {
        self.0.swap_buffers().map_err(Into::into)
    }

    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void
    {
        self.0.get_proc_address(symbol) as *const c_void
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        self.0.get_framebuffer_dimensions()
    }

    fn is_current(&self) -> bool {
        self.0.is_current()
    }

    unsafe fn make_current(&self) {
        self.0.make_current()
    }
}
