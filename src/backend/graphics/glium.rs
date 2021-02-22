//! Glium compatibility module

use crate::backend::graphics::{gl::GLGraphicsBackend, SwapBuffersError};
use glium::{
    backend::{Backend, Context, Facade},
    debug::DebugCallbackBehavior,
    SwapBuffersError as GliumSwapBuffersError,
};
use std::{
    cell::{Cell, Ref, RefCell, RefMut},
    fmt,
    os::raw::c_void,
    rc::Rc,
};

/// Wrapper to expose `Glium` compatibility
pub struct GliumGraphicsBackend<T: GLGraphicsBackend> {
    context: Rc<Context>,
    backend: Rc<InternalBackend<T>>,
    // at least this type is not `Send` or even `Sync`.
    // while there can be multiple Frames, they cannot in parallel call `set_finish`.
    // so a buffer of the last error is sufficient, if always cleared...
    error_channel: Rc<Cell<Option<Box<dyn std::error::Error>>>>,
}

// GLGraphicsBackend is a trait, so we have to impl Debug manually
impl<T: GLGraphicsBackend> fmt::Debug for GliumGraphicsBackend<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct BackendDebug<'a, T: GLGraphicsBackend>(&'a Rc<InternalBackend<T>>);
        impl<'a, T: GLGraphicsBackend> fmt::Debug for BackendDebug<'a, T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let b = &self.0 .0.borrow();
                f.debug_struct("GLGraphicsBackend")
                    .field("framebuffer_dimensions", &b.get_framebuffer_dimensions())
                    .field("is_current", &b.is_current())
                    .field("pixel_format", &b.get_pixel_format())
                    .finish()
            }
        }

        f.debug_struct("GliumGraphicsBackend")
            .field("context", &"...")
            .field("backend", &BackendDebug(&self.backend))
            .field("error_channel", &"...")
            .finish()
    }
}

struct InternalBackend<T: GLGraphicsBackend>(RefCell<T>, Rc<Cell<Option<Box<dyn std::error::Error>>>>);

impl<T: GLGraphicsBackend + 'static> GliumGraphicsBackend<T> {
    fn new(backend: T) -> GliumGraphicsBackend<T> {
        let error_channel = Rc::new(Cell::new(None));
        let internal = Rc::new(InternalBackend(RefCell::new(backend), error_channel.clone()));

        GliumGraphicsBackend {
            // cannot fail
            context: unsafe {
                Context::new(internal.clone(), true, DebugCallbackBehavior::default()).unwrap()
            },
            backend: internal,
            error_channel,
        }
    }

    /// Start drawing on the backbuffer.
    ///
    /// This function returns a [`Frame`], which can be used to draw on it.
    /// When the [`Frame`] is destroyed, the buffers are swapped.
    ///
    /// Note that destroying a [`Frame`] is immediate, even if vsync is enabled.
    #[inline]
    pub fn draw(&self) -> Frame {
        Frame(
            glium::Frame::new(self.context.clone(), self.backend.get_framebuffer_dimensions()),
            self.error_channel.clone(),
        )
    }

    /// Borrow the underlying backend.
    ///
    /// This follows the same semantics as [`std::cell::RefCell`](RefCell::borrow).
    /// Multiple read-only borrows are possible. Borrowing the
    /// backend while there is a mutable reference will panic.
    pub fn borrow(&self) -> Ref<'_, T> {
        self.backend.0.borrow()
    }

    /// Borrow the underlying backend mutably.
    ///
    /// This follows the same semantics as [`std::cell::RefCell`](RefCell::borrow_mut).
    /// Holding any other borrow while trying to borrow the backend
    /// mutably will panic. Note that Glium will borrow the backend
    /// (not mutably) during rendering.
    pub fn borrow_mut(&self) -> RefMut<'_, T> {
        self.backend.0.borrow_mut()
    }
}

impl<T: GLGraphicsBackend> Facade for GliumGraphicsBackend<T> {
    fn get_context(&self) -> &Rc<Context> {
        &self.context
    }
}

impl<T: GLGraphicsBackend + 'static> From<T> for GliumGraphicsBackend<T> {
    fn from(backend: T) -> Self {
        GliumGraphicsBackend::new(backend)
    }
}

unsafe impl<T: GLGraphicsBackend> Backend for InternalBackend<T> {
    fn swap_buffers(&self) -> Result<(), GliumSwapBuffersError> {
        if let Err(err) = self.0.borrow().swap_buffers() {
            Err(match err {
                SwapBuffersError::ContextLost(err) => {
                    self.1.set(Some(err));
                    GliumSwapBuffersError::ContextLost
                }
                SwapBuffersError::TemporaryFailure(err) => {
                    self.1.set(Some(err));
                    GliumSwapBuffersError::AlreadySwapped
                }
                // I do not think, this may happen, but why not
                SwapBuffersError::AlreadySwapped => GliumSwapBuffersError::AlreadySwapped,
            })
        } else {
            Ok(())
        }
    }

    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void {
        self.0.borrow().get_proc_address(symbol) as *const c_void
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        self.0.borrow().get_framebuffer_dimensions()
    }

    fn is_current(&self) -> bool {
        self.0.borrow().is_current()
    }

    unsafe fn make_current(&self) {
        // TODO, if this ever blows up anvil, we should probably silently ignore this.
        // But I have no idea, if that may happen or what glium does, if the context is not current...
        // So lets leave this in to do some real world testing
        self.0.borrow().make_current().expect("Context was lost")
    }
}

/// Implementation of `glium::Surface`, targeting the default framebuffer.
///
/// The back- and front-buffers are swapped when you call `finish`.
///
/// You **must** call either `finish` or `set_finish` or else the destructor will panic.
pub struct Frame(glium::Frame, Rc<Cell<Option<Box<dyn std::error::Error>>>>);

impl Frame {
    /// Stop drawing, swap the buffers, and consume the Frame.
    ///
    /// See the documentation of [`SwapBuffersError`] about what is being returned.
    pub fn finish(mut self) -> Result<(), SwapBuffersError> {
        self.set_finish()
    }

    /// Stop drawing, swap the buffers.
    ///
    /// The Frame can now be dropped regularly. Calling `finish()` or `set_finish()` again will cause `Err(SwapBuffersError::AlreadySwapped)` to be returned.
    pub fn set_finish(&mut self) -> Result<(), SwapBuffersError> {
        let res = self.0.set_finish();
        let err = self.1.take();
        match (res, err) {
            (Ok(()), _) => Ok(()),
            (Err(GliumSwapBuffersError::AlreadySwapped), Some(err)) => {
                Err(SwapBuffersError::TemporaryFailure(err))
            }
            (Err(GliumSwapBuffersError::AlreadySwapped), None) => Err(SwapBuffersError::AlreadySwapped),
            (Err(GliumSwapBuffersError::ContextLost), Some(err)) => Err(SwapBuffersError::ContextLost(err)),
            _ => unreachable!(),
        }
    }
}

impl glium::Surface for Frame {
    fn clear(
        &mut self,
        rect: Option<&glium::Rect>,
        color: Option<(f32, f32, f32, f32)>,
        color_srgb: bool,
        depth: Option<f32>,
        stencil: Option<i32>,
    ) {
        self.0.clear(rect, color, color_srgb, depth, stencil)
    }

    fn get_dimensions(&self) -> (u32, u32) {
        self.0.get_dimensions()
    }

    fn get_depth_buffer_bits(&self) -> Option<u16> {
        self.0.get_depth_buffer_bits()
    }

    fn get_stencil_buffer_bits(&self) -> Option<u16> {
        self.0.get_stencil_buffer_bits()
    }

    fn draw<'a, 'b, V, I, U>(
        &mut self,
        v: V,
        i: I,
        program: &glium::Program,
        uniforms: &U,
        draw_parameters: &glium::draw_parameters::DrawParameters<'_>,
    ) -> Result<(), glium::DrawError>
    where
        V: glium::vertex::MultiVerticesSource<'b>,
        I: Into<glium::index::IndicesSource<'a>>,
        U: glium::uniforms::Uniforms,
    {
        self.0.draw(v, i, program, uniforms, draw_parameters)
    }

    fn blit_from_frame(
        &self,
        source_rect: &glium::Rect,
        target_rect: &glium::BlitTarget,
        filter: glium::uniforms::MagnifySamplerFilter,
    ) {
        self.0.blit_from_frame(source_rect, target_rect, filter);
    }

    fn blit_from_simple_framebuffer(
        &self,
        source: &glium::framebuffer::SimpleFrameBuffer<'_>,
        source_rect: &glium::Rect,
        target_rect: &glium::BlitTarget,
        filter: glium::uniforms::MagnifySamplerFilter,
    ) {
        self.0
            .blit_from_simple_framebuffer(source, source_rect, target_rect, filter)
    }

    fn blit_from_multioutput_framebuffer(
        &self,
        source: &glium::framebuffer::MultiOutputFrameBuffer<'_>,
        source_rect: &glium::Rect,
        target_rect: &glium::BlitTarget,
        filter: glium::uniforms::MagnifySamplerFilter,
    ) {
        self.0
            .blit_from_multioutput_framebuffer(source, source_rect, target_rect, filter)
    }

    fn blit_color<S>(
        &self,
        source_rect: &glium::Rect,
        target: &S,
        target_rect: &glium::BlitTarget,
        filter: glium::uniforms::MagnifySamplerFilter,
    ) where
        S: glium::Surface,
    {
        self.0.blit_color(source_rect, target, target_rect, filter)
    }
}
