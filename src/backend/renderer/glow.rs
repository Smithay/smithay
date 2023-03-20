//! Integration for using [`glow`] on top of smithays OpenGL ES 2 renderer
use tracing::warn;

#[cfg(feature = "wayland_frontend")]
use crate::backend::renderer::{ImportDmaWl, ImportMemWl};
#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
use crate::backend::{egl::display::EGLBufferReader, renderer::ImportEgl};
use crate::{
    backend::{
        allocator::{dmabuf::Dmabuf, Format},
        egl::EGLContext,
        renderer::{
            element::UnderlyingStorage,
            gles2::{element::*, *},
            Bind, Blit, DebugFlags, ExportDma, ExportMem, ImportDma, ImportMem, Offscreen, Renderer,
            TextureFilter, Unbind,
        },
    },
    utils::{Buffer as BufferCoord, Physical, Rectangle, Size, Transform},
};

#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::wl_buffer;

use glow::Context;
use std::{
    borrow::{Borrow, BorrowMut},
    collections::HashSet,
    sync::Arc,
};

use super::{element::RenderElement, Frame};

#[derive(Debug)]
/// A renderer utilizing OpenGL ES 2 and [`glow`] on top for easier custom rendering.
pub struct GlowRenderer {
    gl: Gles2Renderer,
    glow: Arc<Context>,
}

#[derive(Debug)]
/// [`Frame`](super::Frame) implementation of a [`GlowRenderer`].
///
/// Leaking the frame will cause the same problems as leaking a [`Gles2Frame`].
pub struct GlowFrame<'a> {
    frame: Option<Gles2Frame<'a>>,
    glow: Arc<Context>,
}

impl GlowRenderer {
    /// Creates a new OpenGL ES 2 + Glow renderer from a given [`EGLContext`](crate::backend::egl::EGLContext).
    ///
    /// # Safety
    ///
    /// This operation will cause undefined behavior if the given EGLContext is active in another thread.
    ///
    /// # Implementation details
    ///
    /// - Texture handles created by the resulting renderer are valid for every rendered created with an
    /// `EGLContext` shared with the given one (see `EGLContext::new_shared`) and can be used on
    /// any of these renderers.
    /// - This renderer has no default framebuffer, use `Bind::bind` before rendering.
    /// - Binding a new target, while another one is already bound, will replace the current target.
    /// - Shm buffers can be released after a successful import, without the texture handle becoming invalid.
    /// - Texture filtering starts with Linear-downscaling and Linear-upscaling

    pub unsafe fn new(context: EGLContext) -> Result<GlowRenderer, Gles2Error> {
        let glow = {
            context.make_current()?;
            Context::from_loader_function(|s| crate::backend::egl::get_proc_address(s) as *const _)
        };
        let gl = Gles2Renderer::new(context)?;

        Ok(GlowRenderer {
            gl,
            glow: Arc::new(glow),
        })
    }

    /// Get access to the underlying [`EGLContext`].
    ///
    /// *Note*: Modifying the context state, might result in rendering issues.
    /// The context state is considerd an implementation detail
    /// and no guarantee is made about what can or cannot be changed.
    /// To make sure a certain modification does not interfere with
    /// the renderer's behaviour, check the source.
    pub fn egl_context(&self) -> &EGLContext {
        self.gl.egl_context()
    }

    /// Run custom code in the GL context owned by this renderer.
    ///
    /// The OpenGL state of the renderer is considered an implementation detail
    /// and no guarantee is made about what can or cannot be changed,
    /// as such you should reset everything you change back to its previous value
    /// or check the source code of the version of Smithay you are using to ensure
    /// your changes don't interfere with the renderer's behavior.
    /// Doing otherwise can lead to rendering errors while using other functions of this renderer.
    pub fn with_context<F, R>(&mut self, func: F) -> Result<R, Gles2Error>
    where
        F: FnOnce(&Arc<Context>) -> R,
    {
        self.gl.make_current()?;
        Ok(func(&self.glow))
    }
}

impl<'frame> GlowFrame<'frame> {
    /// Run custom code in the GL context owned by this renderer.
    ///
    /// The OpenGL state of the renderer is considered an implementation detail
    /// and no guarantee is made about what can or cannot be changed,
    /// as such you should reset everything you change back to its previous value
    /// or check the source code of the version of Smithay you are using to ensure
    /// your changes don't interfere with the renderer's behavior.
    /// Doing otherwise can lead to rendering errors while using other functions of this renderer.
    pub fn with_context<F, R>(&mut self, func: F) -> Result<R, Gles2Error>
    where
        F: FnOnce(&Arc<Context>) -> R,
    {
        Ok(func(&self.glow))
    }
}

// TODO: When GAT, use TryFrom and be generic over the Error,
//  so `TryFrom<Gles2Renderer, Error=Infaillable> for Gles2Renderer` qualifies
//  just as `TryFrom<Gles2Renderer, Error=Gles2Error> for GlowRenderer`
impl From<Gles2Renderer> for GlowRenderer {
    fn from(mut renderer: Gles2Renderer) -> GlowRenderer {
        let glow = unsafe {
            renderer.make_current().unwrap();
            Context::from_loader_function(|s| crate::backend::egl::get_proc_address(s) as *const _)
        };

        GlowRenderer {
            gl: renderer,
            glow: Arc::new(glow),
        }
    }
}

impl Borrow<Gles2Renderer> for GlowRenderer {
    fn borrow(&self) -> &Gles2Renderer {
        &self.gl
    }
}

impl BorrowMut<Gles2Renderer> for GlowRenderer {
    fn borrow_mut(&mut self) -> &mut Gles2Renderer {
        &mut self.gl
    }
}

impl<'frame> Borrow<Gles2Frame<'frame>> for GlowFrame<'frame> {
    fn borrow(&self) -> &Gles2Frame<'frame> {
        self.frame.as_ref().unwrap()
    }
}

impl<'frame> BorrowMut<Gles2Frame<'frame>> for GlowFrame<'frame> {
    fn borrow_mut(&mut self) -> &mut Gles2Frame<'frame> {
        self.frame.as_mut().unwrap()
    }
}

impl Renderer for GlowRenderer {
    type Error = Gles2Error;
    type TextureId = Gles2Texture;
    type Frame<'frame> = GlowFrame<'frame>;

    fn id(&self) -> usize {
        self.gl.id()
    }

    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.gl.downscale_filter(filter)
    }
    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.gl.upscale_filter(filter)
    }

    fn set_debug_flags(&mut self, flags: DebugFlags) {
        self.gl.set_debug_flags(flags)
    }
    fn debug_flags(&self) -> DebugFlags {
        self.gl.debug_flags()
    }

    fn render(
        &mut self,
        output_size: Size<i32, Physical>,
        transform: Transform,
    ) -> Result<GlowFrame<'_>, Self::Error> {
        let glow = self.glow.clone();
        let frame = self.gl.render(output_size, transform)?;
        Ok(GlowFrame {
            frame: Some(frame),
            glow,
        })
    }
}

impl<'frame> Frame for GlowFrame<'frame> {
    type TextureId = Gles2Texture;
    type Error = Gles2Error;

    fn id(&self) -> usize {
        self.frame.as_ref().unwrap().id()
    }

    fn clear(&mut self, color: [f32; 4], at: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
        self.frame.as_mut().unwrap().clear(color, at)
    }

    fn render_texture_from_to(
        &mut self,
        texture: &Self::TextureId,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        src_transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        Frame::render_texture_from_to(
            self.frame.as_mut().unwrap(),
            texture,
            src,
            dst,
            damage,
            src_transform,
            alpha,
        )
    }

    fn transformation(&self) -> Transform {
        self.frame.as_ref().unwrap().transformation()
    }

    fn render_texture_at(
        &mut self,
        texture: &Self::TextureId,
        pos: crate::utils::Point<i32, Physical>,
        texture_scale: i32,
        output_scale: impl Into<crate::utils::Scale<f64>>,
        src_transform: Transform,
        damage: &[Rectangle<i32, Physical>],
        alpha: f32,
    ) -> Result<(), Self::Error> {
        self.frame.as_mut().unwrap().render_texture_at(
            texture,
            pos,
            texture_scale,
            output_scale,
            src_transform,
            damage,
            alpha,
        )
    }

    fn finish(mut self) -> Result<(), Self::Error> {
        self.finish_internal()
    }
}

impl<'frame> GlowFrame<'frame> {
    fn finish_internal(&mut self) -> Result<(), Gles2Error> {
        if let Some(frame) = self.frame.take() {
            frame.finish()
        } else {
            Ok(())
        }
    }
}

impl<'frame> Drop for GlowFrame<'frame> {
    fn drop(&mut self) {
        if let Err(err) = self.finish_internal() {
            warn!("Ignored error finishing GlowFrame on drop: {}", err);
        }
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportMemWl for GlowRenderer {
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&crate::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, BufferCoord>],
    ) -> Result<Gles2Texture, Gles2Error> {
        self.gl.import_shm_buffer(buffer, surface, damage)
    }
}

impl ImportMem for GlowRenderer {
    fn import_memory(
        &mut self,
        data: &[u8],
        size: Size<i32, BufferCoord>,
        flipped: bool,
    ) -> Result<Gles2Texture, Gles2Error> {
        self.gl.import_memory(data, size, flipped)
    }

    fn update_memory(
        &mut self,
        texture: &<Self as Renderer>::TextureId,
        data: &[u8],
        region: Rectangle<i32, BufferCoord>,
    ) -> Result<(), <Self as Renderer>::Error> {
        self.gl.update_memory(texture, data, region)
    }
}

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
impl ImportEgl for GlowRenderer {
    fn bind_wl_display(
        &mut self,
        display: &wayland_server::DisplayHandle,
    ) -> Result<(), crate::backend::egl::Error> {
        self.gl.bind_wl_display(display)
    }

    fn unbind_wl_display(&mut self) {
        self.gl.unbind_wl_display()
    }

    fn egl_reader(&self) -> Option<&EGLBufferReader> {
        self.gl.egl_reader()
    }

    fn import_egl_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&crate::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, BufferCoord>],
    ) -> Result<Gles2Texture, Gles2Error> {
        self.gl.import_egl_buffer(buffer, surface, damage)
    }
}

impl ImportDma for GlowRenderer {
    fn import_dmabuf(
        &mut self,
        buffer: &Dmabuf,
        damage: Option<&[Rectangle<i32, BufferCoord>]>,
    ) -> Result<Gles2Texture, Gles2Error> {
        self.gl.import_dmabuf(buffer, damage)
    }
    fn dmabuf_formats<'a>(&'a self) -> Box<dyn Iterator<Item = &'a Format> + 'a> {
        self.gl.dmabuf_formats()
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportDmaWl for GlowRenderer {}

impl ExportMem for GlowRenderer {
    type TextureMapping = Gles2Mapping;

    fn copy_framebuffer(
        &mut self,
        region: Rectangle<i32, BufferCoord>,
    ) -> Result<Self::TextureMapping, Self::Error> {
        self.gl.copy_framebuffer(region)
    }

    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, BufferCoord>,
    ) -> Result<Self::TextureMapping, Self::Error> {
        self.gl.copy_texture(texture, region)
    }

    fn map_texture<'a>(
        &mut self,
        texture_mapping: &'a Self::TextureMapping,
    ) -> Result<&'a [u8], Self::Error> {
        self.gl.map_texture(texture_mapping)
    }
}

impl ExportDma for GlowRenderer {
    fn export_texture(&mut self, texture: &Gles2Texture) -> Result<Dmabuf, Gles2Error> {
        self.gl.export_texture(texture)
    }
    fn export_framebuffer(&mut self, size: Size<i32, BufferCoord>) -> Result<Dmabuf, Gles2Error> {
        self.gl.export_framebuffer(size)
    }
}

impl<T> Bind<T> for GlowRenderer
where
    Gles2Renderer: Bind<T>,
{
    fn bind(&mut self, target: T) -> Result<(), Gles2Error> {
        self.gl.bind(target)
    }
    fn supported_formats(&self) -> Option<HashSet<Format>> {
        self.gl.supported_formats()
    }
}

impl<T> Offscreen<T> for GlowRenderer
where
    Gles2Renderer: Offscreen<T>,
{
    fn create_buffer(&mut self, size: Size<i32, BufferCoord>) -> Result<T, Gles2Error> {
        self.gl.create_buffer(size)
    }
}

impl<Target> Blit<Target> for GlowRenderer
where
    Gles2Renderer: Blit<Target>,
{
    fn blit_to(
        &mut self,
        to: Target,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), Gles2Error> {
        self.gl.blit_to(to, src, dst, filter)
    }

    fn blit_from(
        &mut self,
        from: Target,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), Gles2Error> {
        self.gl.blit_from(from, src, dst, filter)
    }
}

impl Unbind for GlowRenderer {
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error> {
        self.gl.unbind()
    }
}

impl RenderElement<GlowRenderer> for PixelShaderElement {
    fn draw<'a>(
        &self,
        frame: &mut GlowFrame<'a>,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), Gles2Error> {
        RenderElement::<Gles2Renderer>::draw(self, frame.borrow_mut(), src, dst, damage)
    }

    fn underlying_storage(&self, renderer: &mut GlowRenderer) -> Option<UnderlyingStorage> {
        RenderElement::<Gles2Renderer>::underlying_storage(self, renderer.borrow_mut())
    }
}
