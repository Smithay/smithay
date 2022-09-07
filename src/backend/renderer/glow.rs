//! Integration for using [`glow`] on top of smithays OpenGL ES 2 renderer

#[cfg(feature = "wayland_frontend")]
use crate::backend::renderer::{ImportDmaWl, ImportMemWl};
#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
use crate::backend::{
    egl::display::{EGLBufferReader, Error as EglError},
    renderer::ImportEgl,
};
use crate::{
    backend::{
        allocator::{dmabuf::Dmabuf, Format},
        egl::{EGLContext, EGLSurface},
        renderer::{
            gles2::*, Bind, ExportDma, ExportMem, ImportDma, ImportMem, Offscreen, Renderer, TextureFilter,
            Unbind,
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
    fmt,
    ops::{Deref, DerefMut},
    rc::Rc,
    sync::Arc,
};

#[derive(Debug)]
/// A renderer utilizing OpenGL ES 2 and [`glow`] on top for easier custom rendering.
pub struct GlowRenderer {
    gl: Gles2Hack,
    glow: Arc<Context>,
    logger: slog::Logger,
}

#[allow(clippy::large_enum_variant)]
enum Gles2Hack {
    Owned(Gles2Renderer),
    Borrowed(*mut Gles2Renderer),
}

impl fmt::Debug for Gles2Hack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Gles2Hack::Owned(renderer) => renderer.fmt(f),
            Gles2Hack::Borrowed(renderer) => unsafe { &**renderer }.fmt(f),
        }
    }
}

impl Deref for Gles2Hack {
    type Target = Gles2Renderer;

    fn deref(&self) -> &Gles2Renderer {
        match self {
            Gles2Hack::Owned(renderer) => renderer,
            Gles2Hack::Borrowed(renderer) => unsafe { &**renderer },
        }
    }
}

impl DerefMut for Gles2Hack {
    fn deref_mut(&mut self) -> &mut Gles2Renderer {
        match self {
            Gles2Hack::Owned(renderer) => renderer,
            Gles2Hack::Borrowed(renderer) => unsafe { &mut **renderer },
        }
    }
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

    pub unsafe fn new<L>(context: EGLContext, logger: L) -> Result<GlowRenderer, Gles2Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger).new(slog::o!("smithay_module" => "renderer_glow"));
        let glow = {
            context.make_current()?;
            Context::from_loader_function(|s| crate::backend::egl::get_proc_address(s) as *const _)
        };
        let gl = Gles2Renderer::new(context, log.clone())?;

        Ok(GlowRenderer {
            gl: Gles2Hack::Owned(gl),
            glow: Arc::new(glow),
            logger: log,
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
        F: FnOnce(&mut Self, &Arc<Context>) -> R,
    {
        self.gl.make_current()?;
        let glow = self.glow.clone();
        Ok(func(self, &glow))
    }
}

// TODO: When GAT, use TryFrom and be generic over the Error,
//  so `TryFrom<Gles2Renderer, Error=Infaillable> for Gles2Renderer` qualifies
//  just as `TryFrom<Gles2Renderer, Error=Gles2Error> for GlowRenderer`
impl From<Gles2Renderer> for GlowRenderer {
    fn from(mut renderer: Gles2Renderer) -> GlowRenderer {
        let log = renderer.logger.new(slog::o!("smithay_module" => "renderer_glow"));
        let glow = unsafe {
            renderer.make_current().unwrap();
            Context::from_loader_function(|s| crate::backend::egl::get_proc_address(s) as *const _)
        };

        GlowRenderer {
            gl: Gles2Hack::Owned(renderer),
            glow: Arc::new(glow),
            logger: log,
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

impl Renderer for GlowRenderer {
    type Error = Gles2Error;
    type TextureId = Gles2Texture;
    type Frame = Gles2Frame;

    fn id(&self) -> usize {
        self.gl.id()
    }

    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.gl.downscale_filter(filter)
    }
    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.gl.upscale_filter(filter)
    }

    fn render<F, R>(
        &mut self,
        output_size: Size<i32, Physical>,
        transform: Transform,
        rendering: F,
    ) -> Result<R, Self::Error>
    where
        F: FnOnce(&mut Self, &mut Self::Frame) -> R,
    {
        self.gl.render(output_size, transform, |gl, frame| {
            rendering(
                &mut GlowRenderer {
                    gl: Gles2Hack::Borrowed(gl as *mut _),
                    glow: self.glow.clone(),
                    logger: self.logger.clone(),
                },
                frame,
            )
        })
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
        self.gl.egl_render()
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

impl Bind<Rc<EGLSurface>> for GlowRenderer {
    fn bind(&mut self, surface: Rc<EGLSurface>) -> Result<(), Gles2Error> {
        self.gl.bind(surface)
    }
    fn supported_formats(&self) -> Option<HashSet<Format>> {
        Bind::<Rc<EGLSurface>>::supported_formats(&*self.gl)
    }
}

impl Bind<Dmabuf> for GlowRenderer {
    fn bind(&mut self, dmabuf: Dmabuf) -> Result<(), Gles2Error> {
        self.gl.bind(dmabuf)
    }
    fn supported_formats(&self) -> Option<HashSet<Format>> {
        Bind::<Dmabuf>::supported_formats(&*self.gl)
    }
}

impl Bind<Gles2Texture> for GlowRenderer {
    fn bind(&mut self, texture: Gles2Texture) -> Result<(), Gles2Error> {
        self.gl.bind(texture)
    }
    fn supported_formats(&self) -> Option<HashSet<Format>> {
        Bind::<Gles2Texture>::supported_formats(&*self.gl)
    }
}

impl Offscreen<Gles2Texture> for GlowRenderer {
    fn create_buffer(&mut self, size: Size<i32, BufferCoord>) -> Result<Gles2Texture, Gles2Error> {
        self.gl.create_buffer(size)
    }
}

impl Bind<Gles2Renderbuffer> for GlowRenderer {
    fn bind(&mut self, renderbuffer: Gles2Renderbuffer) -> Result<(), Gles2Error> {
        self.gl.bind(renderbuffer)
    }
    fn supported_formats(&self) -> Option<HashSet<Format>> {
        Bind::<Gles2Renderbuffer>::supported_formats(&*self.gl)
    }
}

impl Offscreen<Gles2Renderbuffer> for GlowRenderer {
    fn create_buffer(&mut self, size: Size<i32, BufferCoord>) -> Result<Gles2Renderbuffer, Gles2Error> {
        self.gl.create_buffer(size)
    }
}

impl Unbind for GlowRenderer {
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error> {
        self.gl.unbind()
    }
}
