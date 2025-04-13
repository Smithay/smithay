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
        allocator::{dmabuf::Dmabuf, format::FormatSet, Format, Fourcc},
        egl::EGLContext,
        renderer::{
            element::UnderlyingStorage,
            gles::{element::*, *},
            sync, Bind, Blit, BlitFrame, Color32F, DebugFlags, ExportMem, ImportDma, ImportMem, Offscreen,
            Renderer, RendererSuper, TextureFilter,
        },
    },
    utils::{Buffer as BufferCoord, Physical, Rectangle, Size, Transform},
};

#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_buffer, wl_shm};

use glow::Context;
use std::{
    borrow::{Borrow, BorrowMut},
    sync::Arc,
};

use super::{element::RenderElement, ContextId, Frame};

#[derive(Debug)]
/// A renderer utilizing OpenGL ES 2 and [`glow`] on top for easier custom rendering.
pub struct GlowRenderer {
    gl: GlesRenderer,
    glow: Arc<Context>,
}

#[derive(Debug)]
/// [`Frame`] implementation of a [`GlowRenderer`].
///
/// Leaking the frame will cause the same problems as leaking a [`GlesFrame`].
pub struct GlowFrame<'frame, 'buffer> {
    frame: Option<GlesFrame<'frame, 'buffer>>,
    glow: Arc<Context>,
}

impl GlowRenderer {
    /// Get the supported [`Capabilities`](Capability) of the renderer
    ///
    /// # Safety
    ///
    /// This operation will cause undefined behavior if the given EGLContext is active in another thread.
    pub unsafe fn supported_capabilities(context: &EGLContext) -> Result<Vec<Capability>, GlesError> {
        GlesRenderer::supported_capabilities(context)
    }

    /// Creates a new OpenGL ES 2 + Glow renderer from a given [`EGLContext`]
    /// with all [`supported capabilities`](Self::supported_capabilities).
    ///
    /// # Safety
    ///
    /// This operation will cause undefined behavior if the given EGLContext is active in another thread.
    ///
    /// See: [`with_capabilities`](Self::with_capabilities) for more information
    pub unsafe fn new(context: EGLContext) -> Result<GlowRenderer, GlesError> {
        let supported_capabilities = Self::supported_capabilities(&context)?;
        Self::with_capabilities(context, supported_capabilities)
    }

    /// Creates a new OpenGL ES 2 + Glow renderer from a given [`EGLContext`]
    /// with the specified [`Capabilities`](Capability). If a requested [`Capability`] is not supported an
    /// error will be returned.
    ///
    /// # Safety
    ///
    /// This operation will cause undefined behavior if the given EGLContext is active in another thread.
    ///
    /// # Implementation details
    ///
    /// - Texture handles created by the resulting renderer are valid for every rendered created with an
    ///   `EGLContext` shared with the given one (see `EGLContext::new_shared`) and can be used on
    ///   any of these renderers.
    /// - This renderer has no default framebuffer, use `Bind::bind` before rendering.
    /// - Binding a new target, while another one is already bound, will replace the current target.
    /// - Shm buffers can be released after a successful import, without the texture handle becoming invalid.
    /// - Texture filtering starts with Linear-downscaling and Linear-upscaling
    pub unsafe fn with_capabilities(
        context: EGLContext,
        capabilities: impl IntoIterator<Item = Capability>,
    ) -> Result<GlowRenderer, GlesError> {
        let glow = {
            context.make_current()?;
            Context::from_loader_function(|s| crate::backend::egl::get_proc_address(s) as *const _)
        };
        let gl = GlesRenderer::with_capabilities(context, capabilities)?;

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
    pub fn with_context<F, R>(&mut self, func: F) -> Result<R, GlesError>
    where
        F: FnOnce(&Arc<Context>) -> R,
    {
        unsafe {
            self.gl.egl_context().make_current()?;
        }
        Ok(func(&self.glow))
    }
}

impl GlowFrame<'_, '_> {
    /// Run custom code in the GL context owned by this renderer.
    ///
    /// The OpenGL state of the renderer is considered an implementation detail
    /// and no guarantee is made about what can or cannot be changed,
    /// as such you should reset everything you change back to its previous value
    /// or check the source code of the version of Smithay you are using to ensure
    /// your changes don't interfere with the renderer's behavior.
    /// Doing otherwise can lead to rendering errors while using other functions of this renderer.
    pub fn with_context<F, R>(&mut self, func: F) -> Result<R, GlesError>
    where
        F: FnOnce(&Arc<Context>) -> R,
    {
        Ok(func(&self.glow))
    }
}

// TODO: When GAT, use TryFrom and be generic over the Error,
//  so `TryFrom<GlesRenderer, Error=Infaillable> for GlesRenderer` qualifies
//  just as `TryFrom<GlesRenderer, Error=GlesError> for GlowRenderer`
impl From<GlesRenderer> for GlowRenderer {
    #[inline]
    fn from(renderer: GlesRenderer) -> GlowRenderer {
        let glow = unsafe {
            renderer.egl_context().make_current().unwrap();
            Context::from_loader_function(|s| crate::backend::egl::get_proc_address(s) as *const _)
        };

        GlowRenderer {
            gl: renderer,
            glow: Arc::new(glow),
        }
    }
}

impl Borrow<GlesRenderer> for GlowRenderer {
    #[inline]
    fn borrow(&self) -> &GlesRenderer {
        &self.gl
    }
}

impl BorrowMut<GlesRenderer> for GlowRenderer {
    #[inline]
    fn borrow_mut(&mut self) -> &mut GlesRenderer {
        &mut self.gl
    }
}

impl<'frame, 'buffer> Borrow<GlesFrame<'frame, 'buffer>> for GlowFrame<'frame, 'buffer> {
    #[inline]
    fn borrow(&self) -> &GlesFrame<'frame, 'buffer> {
        self.frame.as_ref().unwrap()
    }
}

impl<'frame, 'buffer> BorrowMut<GlesFrame<'frame, 'buffer>> for GlowFrame<'frame, 'buffer> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut GlesFrame<'frame, 'buffer> {
        self.frame.as_mut().unwrap()
    }
}

impl RendererSuper for GlowRenderer {
    type Error = GlesError;
    type TextureId = GlesTexture;
    type Framebuffer<'buffer> = GlesTarget<'buffer>;
    type Frame<'frame, 'buffer>
        = GlowFrame<'frame, 'buffer>
    where
        'buffer: 'frame,
        Self: 'frame;
}

impl Renderer for GlowRenderer {
    fn context_id(&self) -> ContextId<GlesTexture> {
        self.gl.context_id()
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

    #[profiling::function]
    fn render<'frame, 'buffer>(
        &'frame mut self,
        target: &'frame mut GlesTarget<'buffer>,
        output_size: Size<i32, Physical>,
        transform: Transform,
    ) -> Result<GlowFrame<'frame, 'buffer>, Self::Error>
    where
        'buffer: 'frame,
    {
        let glow = self.glow.clone();
        let frame = self.gl.render(target, output_size, transform)?;
        Ok(GlowFrame {
            frame: Some(frame),
            glow,
        })
    }

    #[profiling::function]
    fn wait(&mut self, sync: &sync::SyncPoint) -> Result<(), Self::Error> {
        self.gl.wait(sync)
    }

    #[profiling::function]
    fn cleanup_texture_cache(&mut self) -> Result<(), Self::Error> {
        self.gl.cleanup_texture_cache()
    }
}

impl Frame for GlowFrame<'_, '_> {
    type Error = GlesError;
    type TextureId = GlesTexture;

    fn context_id(&self) -> ContextId<GlesTexture> {
        self.frame.as_ref().unwrap().context_id()
    }

    #[profiling::function]
    fn clear(&mut self, color: Color32F, at: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
        self.frame.as_mut().unwrap().clear(color, at)
    }

    #[profiling::function]
    fn draw_solid(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: Color32F,
    ) -> Result<(), Self::Error> {
        self.frame.as_mut().unwrap().draw_solid(dst, damage, color)
    }

    #[profiling::function]
    fn render_texture_from_to(
        &mut self,
        texture: &Self::TextureId,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        src_transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        Frame::render_texture_from_to(
            self.frame.as_mut().unwrap(),
            texture,
            src,
            dst,
            damage,
            opaque_regions,
            src_transform,
            alpha,
        )
    }

    fn transformation(&self) -> Transform {
        self.frame.as_ref().unwrap().transformation()
    }

    #[profiling::function]
    fn render_texture_at(
        &mut self,
        texture: &Self::TextureId,
        pos: crate::utils::Point<i32, Physical>,
        texture_scale: i32,
        output_scale: impl Into<crate::utils::Scale<f64>>,
        src_transform: Transform,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        alpha: f32,
    ) -> Result<(), Self::Error> {
        self.frame.as_mut().unwrap().render_texture_at(
            texture,
            pos,
            texture_scale,
            output_scale,
            src_transform,
            damage,
            opaque_regions,
            alpha,
        )
    }

    #[profiling::function]
    fn wait(&mut self, sync: &sync::SyncPoint) -> Result<(), Self::Error> {
        self.frame.as_mut().unwrap().wait(sync)
    }

    #[profiling::function]
    fn finish(mut self) -> Result<sync::SyncPoint, Self::Error> {
        self.finish_internal()
    }
}

impl GlowFrame<'_, '_> {
    #[profiling::function]
    fn finish_internal(&mut self) -> Result<sync::SyncPoint, GlesError> {
        if let Some(frame) = self.frame.take() {
            frame.finish()
        } else {
            Ok(sync::SyncPoint::default())
        }
    }
}

impl Drop for GlowFrame<'_, '_> {
    fn drop(&mut self) {
        if let Err(err) = self.finish_internal() {
            warn!("Ignored error finishing GlowFrame on drop: {}", err);
        }
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportMemWl for GlowRenderer {
    #[profiling::function]
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&crate::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, BufferCoord>],
    ) -> Result<GlesTexture, GlesError> {
        self.gl.import_shm_buffer(buffer, surface, damage)
    }

    fn shm_formats(&self) -> Box<dyn Iterator<Item = wl_shm::Format>> {
        self.gl.shm_formats()
    }
}

impl ImportMem for GlowRenderer {
    #[profiling::function]
    fn import_memory(
        &mut self,
        data: &[u8],
        format: Fourcc,
        size: Size<i32, BufferCoord>,
        flipped: bool,
    ) -> Result<GlesTexture, GlesError> {
        self.gl.import_memory(data, format, size, flipped)
    }

    #[profiling::function]
    fn update_memory(
        &mut self,
        texture: &Self::TextureId,
        data: &[u8],
        region: Rectangle<i32, BufferCoord>,
    ) -> Result<(), Self::Error> {
        self.gl.update_memory(texture, data, region)
    }

    fn mem_formats(&self) -> Box<dyn Iterator<Item = Fourcc>> {
        self.gl.mem_formats()
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

    #[profiling::function]
    fn import_egl_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&crate::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, BufferCoord>],
    ) -> Result<GlesTexture, GlesError> {
        self.gl.import_egl_buffer(buffer, surface, damage)
    }
}

impl ImportDma for GlowRenderer {
    #[profiling::function]
    fn import_dmabuf(
        &mut self,
        buffer: &Dmabuf,
        damage: Option<&[Rectangle<i32, BufferCoord>]>,
    ) -> Result<GlesTexture, GlesError> {
        self.gl.import_dmabuf(buffer, damage)
    }
    fn dmabuf_formats(&self) -> FormatSet {
        self.gl.dmabuf_formats()
    }
    fn has_dmabuf_format(&self, format: Format) -> bool {
        self.gl.has_dmabuf_format(format)
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportDmaWl for GlowRenderer {}

impl ExportMem for GlowRenderer {
    type TextureMapping = GlesMapping;

    #[profiling::function]
    fn copy_framebuffer(
        &mut self,
        from: &GlesTarget<'_>,
        region: Rectangle<i32, BufferCoord>,
        format: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        self.gl.copy_framebuffer(from, region, format)
    }

    #[profiling::function]
    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, BufferCoord>,
        format: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        self.gl.copy_texture(texture, region, format)
    }

    fn can_read_texture(&mut self, texture: &Self::TextureId) -> Result<bool, Self::Error> {
        self.gl.can_read_texture(texture)
    }

    #[profiling::function]
    fn map_texture<'a>(
        &mut self,
        texture_mapping: &'a Self::TextureMapping,
    ) -> Result<&'a [u8], Self::Error> {
        self.gl.map_texture(texture_mapping)
    }
}

impl<T> Bind<T> for GlowRenderer
where
    GlesRenderer: Bind<T>,
{
    #[profiling::function]
    fn bind<'a>(&mut self, target: &'a mut T) -> Result<GlesTarget<'a>, GlesError> {
        self.gl.bind(target)
    }
    fn supported_formats(&self) -> Option<FormatSet> {
        self.gl.supported_formats()
    }
}

impl<T> Offscreen<T> for GlowRenderer
where
    GlesRenderer: Offscreen<T>,
{
    #[profiling::function]
    fn create_buffer(&mut self, format: Fourcc, size: Size<i32, BufferCoord>) -> Result<T, GlesError> {
        self.gl.create_buffer(format, size)
    }
}

impl<'buffer> BlitFrame<GlesTarget<'buffer>> for GlowFrame<'_, 'buffer> {
    fn blit_to(
        &mut self,
        to: &mut GlesTarget<'buffer>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), Self::Error> {
        self.frame.as_mut().unwrap().blit_to(to, src, dst, filter)
    }

    fn blit_from(
        &mut self,
        from: &GlesTarget<'buffer>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), Self::Error> {
        self.frame.as_mut().unwrap().blit_from(from, src, dst, filter)
    }
}

impl Blit for GlowRenderer {
    #[profiling::function]
    fn blit(
        &mut self,
        from: &GlesTarget<'_>,
        to: &mut GlesTarget<'_>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), GlesError> {
        self.gl.blit(from, to, src, dst, filter)
    }
}

impl RenderElement<GlowRenderer> for PixelShaderElement {
    #[profiling::function]
    fn draw(
        &self,
        frame: &mut GlowFrame<'_, '_>,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        RenderElement::<GlesRenderer>::draw(self, frame.borrow_mut(), src, dst, damage, opaque_regions)
    }

    fn underlying_storage(&self, renderer: &mut GlowRenderer) -> Option<UnderlyingStorage<'_>> {
        RenderElement::<GlesRenderer>::underlying_storage(self, renderer.borrow_mut())
    }
}

impl RenderElement<GlowRenderer> for TextureShaderElement {
    #[profiling::function]
    fn draw(
        &self,
        frame: &mut GlowFrame<'_, '_>,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        RenderElement::<GlesRenderer>::draw(self, frame.borrow_mut(), src, dst, damage, opaque_regions)
    }

    fn underlying_storage(&self, renderer: &mut GlowRenderer) -> Option<UnderlyingStorage<'_>> {
        RenderElement::<GlesRenderer>::underlying_storage(self, renderer.borrow_mut())
    }
}
