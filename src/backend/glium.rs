use glium::backend::Backend;
use glium::SwapBuffersError as GliumSwapBuffersError;

use ::backend::graphics::opengl::{OpenglRenderer, SwapBuffersError};

impl From<SwapBuffersError> for GliumSwapBuffersError
{
    fn from(error: SwapBuffersError) -> Self {
        match error {
            SwapBuffersError::ContextLost => GliumSwapBuffersError::ContextLost,
            SwapBuffersError::AlreadySwapped => GliumSwapBuffersError::AlreadySwapped,
        }
    }
}

impl<T: OpenglRenderer> Backend for T
{
    fn swap_buffers(&self) -> Result<(), SwapBuffersError> {
        self.swap_buffers().map_err(|x| x.into)
    }

    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void
    {
        self.get_proc_address(symbol)
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        self.get_framebuffer_dimensions()
    }

    fn is_current(&self) -> bool {
        self.is_current()
    }

    unsafe fn make_current(&self) {
        self.make_current()
    }
}
