use std::{mem, sync::atomic::AtomicBool};

use cgmath::{Matrix3, SquareMatrix};
use tracing::{span, Level};
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm};
#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
use wayland_server::DisplayHandle;

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
use crate::backend::{egl::display::EGLBufferReader, renderer::ImportEgl};
#[cfg(feature = "wayland_frontend")]
use crate::{
    backend::renderer::{ImportDmaWl, ImportMemWl},
    utils::{Buffer, Rectangle},
    wayland::compositor::SurfaceData,
};
use crate::{
    backend::{
        allocator::{dmabuf::Dmabuf, format::FormatSet},
        egl::EGLSurface,
        renderer::{
            gles::{
                ffi, GlesError, GlesFrame, GlesMapping, GlesParent, GlesRenderbuffer, GlesRenderer,
                GlesTarget, GlesTexture,
            },
            sync::SyncPoint,
            Bind, Blit, ContextId, DebugFlags, ExportMem, ImportDma, ImportMem, Offscreen, Renderer,
            RendererSuper, TextureFilter,
        },
    },
    gpu_span_location,
    utils::{Physical, Size, Transform},
};

impl<'oldframe, 'oldbuffer> RendererSuper for GlesFrame<'oldframe, 'oldbuffer, '_> {
    type Error = GlesError;
    type TextureId = GlesTexture;
    type Framebuffer<'buffer> = GlesTarget<'buffer>;
    type Frame<'frame, 'buffer>
        = GlesFrame<'frame, 'buffer, 'oldbuffer>
    where
        'buffer: 'frame,
        Self: 'frame;
}

impl<'oldframe, 'oldbuffer> Renderer for GlesFrame<'oldframe, 'oldbuffer, '_> {
    fn context_id(&self) -> ContextId<Self::TextureId> {
        self.parent.renderer().context_id()
    }

    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        // TODO: Apply locally
        self.parent.renderer_mut().downscale_filter(filter)
    }

    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        // TODO: Apply locally
        self.parent.renderer_mut().upscale_filter(filter)
    }

    fn set_debug_flags(&mut self, flags: DebugFlags) {
        // TODO: Apply locally
        self.parent.renderer_mut().set_debug_flags(flags)
    }

    fn debug_flags(&self) -> DebugFlags {
        self.parent.renderer().debug_flags()
    }

    fn render<'frame, 'buffer>(
        &'frame mut self,
        target: &'frame mut Self::Framebuffer<'buffer>,
        mut output_size: Size<i32, Physical>,
        transform: Transform,
    ) -> Result<GlesFrame<'frame, 'buffer, 'oldbuffer>, Self::Error>
    where
        'buffer: 'frame,
        'oldframe: 'frame,
    {
        let renderer = self.parent.renderer();
        target.0.make_current(&renderer.gl, &renderer.egl)?;

        let gpu_span = renderer
            .profiler
            .enter(gpu_span_location!("render"), &renderer.gl);

        unsafe {
            renderer.gl.Viewport(0, 0, output_size.w, output_size.h);

            renderer.gl.Scissor(0, 0, output_size.w, output_size.h);
            renderer.gl.Enable(ffi::SCISSOR_TEST);

            renderer.gl.Enable(ffi::BLEND);
            renderer.gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
        }

        // Handle the width/height swap when the output is rotated by 90°/270°.
        if let Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270 = transform {
            mem::swap(&mut output_size.w, &mut output_size.h);
        }

        // replicate https://www.khronos.org/registry/OpenGL-Refpages/gl2.1/xhtml/glOrtho.xml
        // glOrtho(0, width, 0, height, 1, 1);
        let mut renderer = Matrix3::<f32>::identity();
        let t = Matrix3::<f32>::identity();
        let x = 2.0 / (output_size.w as f32);
        let y = 2.0 / (output_size.h as f32);

        // Rotation & Reflection
        renderer[0][0] = x * t[0][0];
        renderer[1][0] = x * t[0][1];
        renderer[0][1] = y * -t[1][0];
        renderer[1][1] = y * -t[1][1];

        //Translation
        renderer[2][0] = -(1.0f32.copysign(renderer[0][0] + renderer[1][0]));
        renderer[2][1] = -(1.0f32.copysign(renderer[0][1] + renderer[1][1]));

        // We account for OpenGLs coordinate system here
        let flip180 = Matrix3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0);

        let current_projection = flip180 * transform.matrix() * renderer;
        let span = span!(parent: &self.span, Level::DEBUG, "renderer_gles2_frame", current_projection = ?current_projection, size = ?output_size, transform = ?transform).entered();

        let renderer: *mut GlesRenderer = match &self.parent {
            GlesParent::Renderer(renderer) | GlesParent::Frame { renderer, .. } => *renderer,
            _ => unreachable!(),
        };

        Ok(GlesFrame {
            parent: GlesParent::Frame {
                renderer,
                target: self.target as *mut _,
                old_size: self.size,
                old_transform: self.transform,
            },
            target,
            // output transformation passed in by the user
            current_projection,
            transform,
            size: output_size,
            tex_program_override: None,
            finished: AtomicBool::new(false),

            span,

            gpu_span: Some(gpu_span),
        })
    }

    fn wait(&mut self, sync: &SyncPoint) -> Result<(), Self::Error> {
        self.parent.renderer_mut().wait(sync)
    }

    fn cleanup_texture_cache(&mut self) -> Result<(), Self::Error> {
        self.parent.renderer_mut().cleanup_texture_cache()
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportMemWl for GlesFrame<'_, '_, '_> {
    fn import_shm_buffer(
        &mut self,
        buffer: &WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<Self::TextureId, Self::Error> {
        let res = self
            .parent
            .renderer_mut()
            .import_shm_buffer(buffer, surface, damage);
        // TODO: Refactor to skip the GlesRenderers make_current call
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }

    fn shm_formats(&self) -> Box<dyn Iterator<Item = wl_shm::Format>> {
        self.parent.renderer().shm_formats()
    }
}

impl ImportMem for GlesFrame<'_, '_, '_> {
    fn import_memory(
        &mut self,
        data: &[u8],
        format: gbm::Format,
        size: Size<i32, Buffer>,
        flipped: bool,
    ) -> Result<Self::TextureId, Self::Error> {
        let res = self
            .parent
            .renderer_mut()
            .import_memory(data, format, size, flipped);
        // TODO: Refactor to skip the GlesRenderers make_current call
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }

    fn update_memory(
        &mut self,
        texture: &Self::TextureId,
        data: &[u8],
        region: Rectangle<i32, Buffer>,
    ) -> Result<(), Self::Error> {
        let res = self.parent.renderer_mut().update_memory(texture, data, region);
        // TODO: Refactor to skip the GlesRenderers make_current call
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }

    fn mem_formats(&self) -> Box<dyn Iterator<Item = gbm::Format>> {
        self.parent.renderer().mem_formats()
    }
}

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
impl ImportEgl for GlesFrame<'_, '_, '_> {
    fn bind_wl_display(&mut self, display: &DisplayHandle) -> Result<(), crate::backend::egl::Error> {
        self.parent.renderer_mut().bind_wl_display(display)
    }

    fn unbind_wl_display(&mut self) {
        self.parent.renderer_mut().unbind_wl_display();
    }

    fn egl_reader(&self) -> Option<&EGLBufferReader> {
        self.parent.renderer().egl_reader()
    }

    fn import_egl_buffer(
        &mut self,
        buffer: &WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<Self::TextureId, Self::Error> {
        let res = self
            .parent
            .renderer_mut()
            .import_egl_buffer(buffer, surface, damage);
        // TODO: Refactor to skip the GlesRenderers make_current call
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }
}

impl ImportDma for GlesFrame<'_, '_, '_> {
    fn import_dmabuf(
        &mut self,
        dmabuf: &Dmabuf,
        damage: Option<&[Rectangle<i32, Buffer>]>,
    ) -> Result<Self::TextureId, Self::Error> {
        let res = self.parent.renderer_mut().import_dmabuf(dmabuf, damage);
        // TODO: Refactor to skip the GlesRenderers make_current call
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportDmaWl for GlesFrame<'_, '_, '_> {
    fn import_dma_buffer(
        &mut self,
        buffer: &WlBuffer,
        surface: Option<&SurfaceData>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<Self::TextureId, Self::Error> {
        let res = self
            .parent
            .renderer_mut()
            .import_dma_buffer(buffer, surface, damage);
        // TODO: Refactor to skip the GlesRenderers make_current call
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }
}

// TODO: ExportMemFrame trait?
impl ExportMem for GlesFrame<'_, '_, '_> {
    type TextureMapping = GlesMapping;

    fn copy_framebuffer(
        &mut self,
        target: &Self::Framebuffer<'_>,
        region: Rectangle<i32, Buffer>,
        format: gbm::Format,
    ) -> Result<Self::TextureMapping, Self::Error> {
        let res = self
            .parent
            .renderer_mut()
            .copy_framebuffer(target, region, format);
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }

    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, Buffer>,
        format: gbm::Format,
    ) -> Result<Self::TextureMapping, Self::Error> {
        let res = self.parent.renderer_mut().copy_texture(texture, region, format);
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }

    fn can_read_texture(&mut self, texture: &Self::TextureId) -> Result<bool, Self::Error> {
        let res = self.parent.renderer_mut().can_read_texture(texture);
        // the GlesRenderer binds the texture to test, if we can read it
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }

    fn map_texture<'a>(
        &mut self,
        texture_mapping: &'a Self::TextureMapping,
    ) -> Result<&'a [u8], Self::Error> {
        let res = self.parent.renderer_mut().map_texture(texture_mapping);
        // TODO: Refactor to skip the GlesRenderers make_current call
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }
}

impl Bind<EGLSurface> for GlesFrame<'_, '_, '_> {
    fn bind<'a>(&mut self, target: &'a mut EGLSurface) -> Result<Self::Framebuffer<'a>, Self::Error> {
        self.parent.renderer_mut().bind(target)
    }

    fn supported_formats(&self) -> Option<FormatSet> {
        Bind::<EGLSurface>::supported_formats(self.parent.renderer())
    }
}

impl Bind<Dmabuf> for GlesFrame<'_, '_, '_> {
    fn bind<'a>(&mut self, target: &'a mut Dmabuf) -> Result<Self::Framebuffer<'a>, Self::Error> {
        let res = self.parent.renderer_mut().bind(target);
        // TODO: Refactor to skip the GlesRenderers make_current call
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }

    fn supported_formats(&self) -> Option<FormatSet> {
        Bind::<Dmabuf>::supported_formats(self.parent.renderer())
    }
}

impl Bind<GlesTexture> for GlesFrame<'_, '_, '_> {
    fn bind<'a>(&mut self, target: &'a mut GlesTexture) -> Result<Self::Framebuffer<'a>, Self::Error> {
        let res = self.parent.renderer_mut().bind(target);
        // TODO: Refactor to skip the GlesRenderers make_current call
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }

    fn supported_formats(&self) -> Option<FormatSet> {
        Bind::<GlesTexture>::supported_formats(self.parent.renderer())
    }
}

impl Bind<GlesRenderbuffer> for GlesFrame<'_, '_, '_> {
    fn bind<'a>(&mut self, target: &'a mut GlesRenderbuffer) -> Result<Self::Framebuffer<'a>, Self::Error> {
        let res = self.parent.renderer_mut().bind(target);
        // TODO: Refactor to skip the GlesRenderers make_current call
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }

    fn supported_formats(&self) -> Option<FormatSet> {
        Bind::<GlesRenderbuffer>::supported_formats(self.parent.renderer())
    }
}

impl Offscreen<GlesTexture> for GlesFrame<'_, '_, '_> {
    fn create_buffer(
        &mut self,
        format: gbm::Format,
        size: Size<i32, Buffer>,
    ) -> Result<GlesTexture, Self::Error> {
        let res = self.parent.renderer_mut().create_buffer(format, size);
        // TODO: Refactor to skip the GlesRenderers make_current call
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }
}

impl Offscreen<GlesRenderbuffer> for GlesFrame<'_, '_, '_> {
    fn create_buffer(
        &mut self,
        format: gbm::Format,
        size: Size<i32, Buffer>,
    ) -> Result<GlesRenderbuffer, Self::Error> {
        let res = self.parent.renderer_mut().create_buffer(format, size);
        // TODO: Refactor to skip the GlesRenderers make_current call
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }
}

impl Blit for GlesFrame<'_, '_, '_> {
    fn blit(
        &mut self,
        from: &Self::Framebuffer<'_>,
        to: &mut Self::Framebuffer<'_>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<SyncPoint, Self::Error> {
        let res = self.parent.renderer_mut().blit(from, to, src, dst, filter);
        let renderer = self.parent.renderer();
        self.target.0.make_current(&renderer.gl, &renderer.egl)?;
        res
    }
}
