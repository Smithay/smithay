use drm::control::{connector, crtc, Mode};
use nix::libc::c_void;

use super::Error;
use crate::backend::drm::Surface;
use crate::backend::egl::native::NativeSurface;
use crate::backend::egl::{get_proc_address, native, EGLContext, EGLSurface};
#[cfg(feature = "renderer_gl")]
use crate::backend::graphics::gl::GLGraphicsBackend;
#[cfg(feature = "renderer_gl")]
use crate::backend::graphics::PixelFormat;
use crate::backend::graphics::{CursorBackend, SwapBuffersError};

/// Egl surface for rendering
pub struct EglSurface<N>
where
    N: native::NativeSurface + Surface,
{
    pub(super) context: EGLContext,
    pub(super) surface: EGLSurface<N>,
}

impl<N> Surface for EglSurface<N>
where
    N: NativeSurface + Surface,
{
    type Connectors = <N as Surface>::Connectors;
    type Error = Error<<N as Surface>::Error>;

    fn crtc(&self) -> crtc::Handle {
        (*self.surface).crtc()
    }

    fn current_connectors(&self) -> Self::Connectors {
        self.surface.current_connectors()
    }

    fn pending_connectors(&self) -> Self::Connectors {
        self.surface.pending_connectors()
    }

    fn add_connector(&self, connector: connector::Handle) -> Result<(), Self::Error> {
        self.surface.add_connector(connector).map_err(Error::Underlying)
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<(), Self::Error> {
        self.surface
            .remove_connector(connector)
            .map_err(Error::Underlying)
    }

    fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Self::Error> {
        self.surface.set_connectors(connectors).map_err(Error::Underlying)
    }

    fn current_mode(&self) -> Option<Mode> {
        self.surface.current_mode()
    }

    fn pending_mode(&self) -> Option<Mode> {
        self.surface.pending_mode()
    }

    fn use_mode(&self, mode: Option<Mode>) -> Result<(), Self::Error> {
        self.surface.use_mode(mode).map_err(Error::Underlying)
    }
}

impl<N> CursorBackend for EglSurface<N>
where
    N: NativeSurface + Surface + CursorBackend,
{
    type CursorFormat = <N as CursorBackend>::CursorFormat;
    type Error = <N as CursorBackend>::Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> ::std::result::Result<(), Self::Error> {
        self.surface.set_cursor_position(x, y)
    }

    fn set_cursor_representation(
        &self,
        buffer: &Self::CursorFormat,
        hotspot: (u32, u32),
    ) -> ::std::result::Result<(), Self::Error> {
        self.surface.set_cursor_representation(buffer, hotspot)
    }
}

#[cfg(feature = "renderer_gl")]
impl<N> GLGraphicsBackend for EglSurface<N>
where
    N: native::NativeSurface + Surface,
{
    fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError> {
        self.surface.swap_buffers()
    }

    fn get_proc_address(&self, symbol: &str) -> *const c_void {
        get_proc_address(symbol)
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        let (w, h) = self.pending_mode().map(|mode| mode.size()).unwrap_or((1, 1));
        (w as u32, h as u32)
    }

    fn is_current(&self) -> bool {
        self.context.is_current() && self.surface.is_current()
    }

    unsafe fn make_current(&self) -> ::std::result::Result<(), SwapBuffersError> {
        self.context.make_current_with_surface(&self.surface)
    }

    fn get_pixel_format(&self) -> PixelFormat {
        self.surface.get_pixel_format()
    }
}
