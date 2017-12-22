//! Glium compatibility module

use backend::graphics::egl::{EGLGraphicsBackend, SwapBuffersError};
use glium::Frame;
use glium::SwapBuffersError as GliumSwapBuffersError;
use glium::backend::{Backend, Context, Facade};
use glium::debug::DebugCallbackBehavior;
use std::borrow::Borrow;
use std::os::raw::c_void;
use std::rc::Rc;

impl From<SwapBuffersError> for GliumSwapBuffersError {
    fn from(error: SwapBuffersError) -> Self {
        match error {
            SwapBuffersError::ContextLost => GliumSwapBuffersError::ContextLost,
            SwapBuffersError::AlreadySwapped => GliumSwapBuffersError::AlreadySwapped,
            SwapBuffersError::Unknown(_) => GliumSwapBuffersError::ContextLost, // TODO
        }
    }
}

/// Wrapper to expose `glium` compatibility
pub struct GliumGraphicsBackend<T: EGLGraphicsBackend> {
    context: Rc<Context>,
    backend: Rc<InternalBackend<T>>,
}

struct InternalBackend<T: EGLGraphicsBackend>(T);

impl<T: EGLGraphicsBackend + 'static> GliumGraphicsBackend<T> {
    fn new(backend: T) -> GliumGraphicsBackend<T> {
        let internal = Rc::new(InternalBackend(backend));

        GliumGraphicsBackend {
            // cannot fail
            context: unsafe {
                Context::new(internal.clone(), true, DebugCallbackBehavior::default()).unwrap()
            },
            backend: internal,
        }
    }

    /// Start drawing on the backbuffer.
    ///
    /// This function returns a `Frame`, which can be used to draw on it. When the `Frame` is
    /// destroyed, the buffers are swapped.
    ///
    /// Note that destroying a `Frame` is immediate, even if vsync is enabled.
    #[inline]
    pub fn draw(&self) -> Frame {
        Frame::new(
            self.context.clone(),
            self.backend.get_framebuffer_dimensions(),
        )
    }
}

impl<T: EGLGraphicsBackend> Borrow<T> for GliumGraphicsBackend<T> {
    fn borrow(&self) -> &T {
        &self.backend.0
    }
}

impl<T: EGLGraphicsBackend> Facade for GliumGraphicsBackend<T> {
    fn get_context(&self) -> &Rc<Context> {
        &self.context
    }
}

impl<T: EGLGraphicsBackend + 'static> From<T> for GliumGraphicsBackend<T> {
    fn from(backend: T) -> Self {
        GliumGraphicsBackend::new(backend)
    }
}

unsafe impl<T: EGLGraphicsBackend> Backend for InternalBackend<T> {
    fn swap_buffers(&self) -> Result<(), GliumSwapBuffersError> {
        self.0.swap_buffers().map_err(Into::into)
    }

    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void {
        self.0.get_proc_address(symbol) as *const c_void
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        self.0.get_framebuffer_dimensions()
    }

    fn is_current(&self) -> bool {
        self.0.is_current()
    }

    unsafe fn make_current(&self) {
        self.0.make_current().expect("Context was lost")
    }
}
