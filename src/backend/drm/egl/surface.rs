use drm::control::{connector, crtc, Mode};
use nix::libc::c_void;
use std::rc::Rc;

use super::error::*;
use backend::drm::{Device, Surface};
use backend::egl::native::{Backend, NativeDisplay, NativeSurface};
use backend::egl::{EGLContext, EGLSurface};
#[cfg(feature = "renderer_gl")]
use backend::graphics::gl::GLGraphicsBackend;
#[cfg(feature = "renderer_gl")]
use backend::graphics::PixelFormat;
use backend::graphics::{CursorBackend, SwapBuffersError};

/// Egl surface for rendering
pub struct EglSurface<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B> + 'static,
    <D as Device>::Surface: NativeSurface,
{
    pub(super) dev: Rc<EGLContext<B, D>>,
    pub(super) surface: EGLSurface<B::Surface>,
}

impl<B, D> Surface for EglSurface<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B> + 'static,
    <D as Device>::Surface: NativeSurface,
{
    type Error = Error;
    type Connectors = <<D as Device>::Surface as Surface>::Connectors;

    fn crtc(&self) -> crtc::Handle {
        (*self.surface).crtc()
    }

    fn current_connectors(&self) -> Self::Connectors {
        self.surface.current_connectors()
    }

    fn pending_connectors(&self) -> Self::Connectors {
        self.surface.pending_connectors()
    }

    fn add_connector(&self, connector: connector::Handle) -> Result<()> {
        self.surface
            .add_connector(connector)
            .chain_err(|| ErrorKind::UnderlyingBackendError)
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<()> {
        self.surface
            .remove_connector(connector)
            .chain_err(|| ErrorKind::UnderlyingBackendError)
    }

    fn current_mode(&self) -> Option<Mode> {
        self.surface.current_mode()
    }

    fn pending_mode(&self) -> Option<Mode> {
        self.surface.pending_mode()
    }

    fn use_mode(&self, mode: Option<Mode>) -> Result<()> {
        self.surface
            .use_mode(mode)
            .chain_err(|| ErrorKind::UnderlyingBackendError)
    }
}

impl<'a, B, D> CursorBackend<'a> for EglSurface<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B> + 'static,
    <D as Device>::Surface: NativeSurface + CursorBackend<'a>,
{
    type CursorFormat = <D::Surface as CursorBackend<'a>>::CursorFormat;
    type Error = <D::Surface as CursorBackend<'a>>::Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> ::std::result::Result<(), Self::Error> {
        self.surface.set_cursor_position(x, y)
    }

    fn set_cursor_representation<'b>(
        &'b self,
        buffer: Self::CursorFormat,
        hotspot: (u32, u32),
    ) -> ::std::result::Result<(), Self::Error>
    where
        'a: 'b,
    {
        self.surface.set_cursor_representation(buffer, hotspot)
    }
}

#[cfg(feature = "renderer_gl")]
impl<B, D> GLGraphicsBackend for EglSurface<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device + NativeDisplay<B> + 'static,
    <D as Device>::Surface: NativeSurface,
{
    fn swap_buffers(&self) -> ::std::result::Result<(), SwapBuffersError> {
        self.surface.swap_buffers()
    }

    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void {
        self.dev.get_proc_address(symbol)
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        let (w, h) = self.pending_mode().map(|mode| mode.size()).unwrap_or((1, 1));
        (w as u32, h as u32)
    }

    fn is_current(&self) -> bool {
        self.dev.is_current() && self.surface.is_current()
    }

    unsafe fn make_current(&self) -> ::std::result::Result<(), SwapBuffersError> {
        self.surface.make_current()
    }

    fn get_pixel_format(&self) -> PixelFormat {
        self.dev.get_pixel_format()
    }
}
