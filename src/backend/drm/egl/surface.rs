use drm::control::{connector, crtc, Mode};
use nix::libc::c_void;
use std::convert::TryInto;

use super::Error;
use crate::backend::drm::Surface;
use crate::backend::egl::native::NativeSurface;
use crate::backend::egl::{get_proc_address, native, EGLContext, EGLSurface};
#[cfg(feature = "renderer_gl")]
use crate::backend::graphics::gl::GLGraphicsBackend;
#[cfg(feature = "renderer_gl")]
use crate::backend::graphics::PixelFormat;
use crate::backend::graphics::{CursorBackend, SwapBuffersError};

use std::rc::Rc;

/// Egl surface for rendering
pub struct EglSurface<N: native::NativeSurface + Surface>(pub(super) Rc<EglSurfaceInternal<N>>);

pub(super) struct EglSurfaceInternal<N>
where
    N: native::NativeSurface + Surface,
{
    pub(super) context: EGLContext,
    pub(super) surface: EGLSurface<N>,
}

impl<N> Surface for EglSurface<N>
where
    N: native::NativeSurface + Surface,
{
    type Connectors = <N as Surface>::Connectors;
    type Error = Error<<N as Surface>::Error>;

    fn crtc(&self) -> crtc::Handle {
        (*self.0.surface).crtc()
    }

    fn current_connectors(&self) -> Self::Connectors {
        self.0.surface.current_connectors()
    }

    fn pending_connectors(&self) -> Self::Connectors {
        self.0.surface.pending_connectors()
    }

    fn add_connector(&self, connector: connector::Handle) -> Result<(), Self::Error> {
        self.0.surface.add_connector(connector).map_err(Error::Underlying)
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<(), Self::Error> {
        self.0
            .surface
            .remove_connector(connector)
            .map_err(Error::Underlying)
    }

    fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Self::Error> {
        self.0
            .surface
            .set_connectors(connectors)
            .map_err(Error::Underlying)
    }

    fn current_mode(&self) -> Mode {
        self.0.surface.current_mode()
    }

    fn pending_mode(&self) -> Mode {
        self.0.surface.pending_mode()
    }

    fn use_mode(&self, mode: Mode) -> Result<(), Self::Error> {
        self.0.surface.use_mode(mode).map_err(Error::Underlying)
    }
}

impl<N> CursorBackend for EglSurface<N>
where
    N: NativeSurface + Surface + CursorBackend,
{
    type CursorFormat = <N as CursorBackend>::CursorFormat;
    type Error = <N as CursorBackend>::Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> ::std::result::Result<(), Self::Error> {
        self.0.surface.set_cursor_position(x, y)
    }

    fn set_cursor_representation(
        &self,
        buffer: &Self::CursorFormat,
        hotspot: (u32, u32),
    ) -> ::std::result::Result<(), Self::Error> {
        self.0.surface.set_cursor_representation(buffer, hotspot)
    }
}

#[cfg(feature = "renderer_gl")]
impl<N> GLGraphicsBackend for EglSurface<N>
where
    N: native::NativeSurface + Surface,
    <N as NativeSurface>::Error: Into<SwapBuffersError> + 'static,
{
    fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError> {
        if let Err(err) = self.0.surface.swap_buffers() {
            Err(match err.try_into() {
                Ok(x) => x,
                Err(x) => x.into(),
            })
        } else {
            Ok(())
        }
    }

    fn get_proc_address(&self, symbol: &str) -> *const c_void {
        get_proc_address(symbol)
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        let (w, h) = self.pending_mode().size();
        (w as u32, h as u32)
    }

    fn is_current(&self) -> bool {
        self.0.context.is_current() && self.0.surface.is_current()
    }

    unsafe fn make_current(&self) -> ::std::result::Result<(), SwapBuffersError> {
        self.0
            .context
            .make_current_with_surface(&self.0.surface)
            .map_err(Into::into)
    }

    fn get_pixel_format(&self) -> PixelFormat {
        self.0.surface.get_pixel_format()
    }
}
