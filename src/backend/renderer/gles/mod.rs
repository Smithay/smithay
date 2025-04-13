//! Implementation of the rendering traits using OpenGL ES 2

use cgmath::{prelude::*, Matrix3, Vector2};
use core::slice;
use std::{
    collections::HashMap,
    ffi::{CStr, CString},
    fmt,
    marker::PhantomData,
    mem,
    os::raw::c_char,
    ptr,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicPtr, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc, Mutex, RwLock, RwLockWriteGuard,
    },
};
use tracing::{debug, error, info, info_span, instrument, span, span::EnteredSpan, trace, warn, Level};

pub mod element;
mod error;
pub mod format;
mod shaders;
mod texture;
mod uniform;
mod version;

pub use error::*;
use format::*;
pub use shaders::*;
pub use texture::*;
pub use uniform::*;

use self::version::GlVersion;

use super::{
    sync::SyncPoint, Bind, Blit, BlitFrame, Color32F, ContextId, DebugFlags, ExportMem, Frame, ImportDma,
    ImportMem, Offscreen, Renderer, RendererSuper, Texture, TextureFilter, TextureMapping,
};
use crate::{
    backend::{
        allocator::{
            dmabuf::{Dmabuf, WeakDmabuf},
            format::{get_bpp, get_opaque, has_alpha, FormatSet},
            Buffer, Format, Fourcc,
        },
        egl::{
            fence::EGLFence,
            ffi::egl::{self as ffi_egl, types::EGLImage},
            EGLContext, EGLSurface, MakeCurrentError,
        },
    },
    utils::{Buffer as BufferCoord, Physical, Rectangle, Size, Transform},
};

#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use super::ImportEgl;
#[cfg(feature = "wayland_frontend")]
use super::{ImportDmaWl, ImportMemWl};
#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use crate::backend::egl::{display::EGLBufferReader, Format as EGLFormat};
#[cfg(feature = "wayland_frontend")]
use crate::wayland::shm::shm_format_to_fourcc;
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::wl_buffer;

#[allow(clippy::all, missing_docs, missing_debug_implementations)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/gl_bindings.rs"));
}

enum CleanupResource {
    Texture(ffi::types::GLuint),
    FramebufferObject(ffi::types::GLuint),
    RenderbufferObject(ffi::types::GLuint),
    EGLImage(EGLImage),
    Mapping(ffi::types::GLuint, *const std::ffi::c_void),
    Program(ffi::types::GLuint),
    Sync(ffi::types::GLsync),
}
unsafe impl Send for CleanupResource {}

#[derive(Debug, Clone)]
struct GlesBuffer {
    dmabuf: WeakDmabuf,
    image: EGLImage,
    rbo: ffi::types::GLuint,
    fbo: ffi::types::GLuint,
}

/// Offscreen render surface
///
/// Usually more performant than using a texture as a framebuffer.
/// Can be read out, but not used like a texture otherwise.
#[derive(Debug, Clone)]
pub struct GlesRenderbuffer(Rc<GlesRenderbufferInternal>);

#[derive(Debug)]
struct GlesRenderbufferInternal {
    rbo: ffi::types::GLuint,
    format: ffi::types::GLenum,
    has_alpha: bool,
    size: Size<i32, BufferCoord>,
    destruction_callback_sender: Sender<CleanupResource>,
}

impl GlesRenderbuffer {
    /// Size of the renderbuffer
    pub fn size(&self) -> Size<i32, BufferCoord> {
        self.0.size
    }

    /// Internal format of the renderbuffer
    pub fn format(&self) -> Option<Fourcc> {
        let fmt = gl_internal_format_to_fourcc(self.0.format);
        if self.0.has_alpha {
            fmt
        } else {
            fmt.and_then(get_opaque)
        }
    }
}

impl Drop for GlesRenderbufferInternal {
    fn drop(&mut self) {
        let _ = self
            .destruction_callback_sender
            .send(CleanupResource::RenderbufferObject(self.rbo));
    }
}

/// A GL framebuffer
#[derive(Debug)]
pub struct GlesTarget<'a>(GlesTargetInternal<'a>);

#[derive(Debug)]
enum GlesTargetInternal<'a> {
    Image {
        // TODO: Ideally we would be able to share the texture between renderers with shared EGLContexts though.
        // But we definitly don't want to add user data to a dmabuf to facilitate this. Maybe use the EGLContexts userdata for storing the buffers?
        buf: GlesBuffer,
        dmabuf: &'a mut Dmabuf,
    },
    Surface {
        surface: &'a mut EGLSurface,
    },
    Texture {
        texture: GlesTexture,
        sync_lock: RwLockWriteGuard<'a, TextureSync>,
        fbo: ffi::types::GLuint,
        destruction_callback_sender: Sender<CleanupResource>,
    },
    Renderbuffer {
        buf: &'a mut GlesRenderbuffer,
        fbo: ffi::types::GLuint,
    },
}

impl Texture for GlesTarget<'_> {
    fn height(&self) -> u32 {
        self.size().h as u32
    }

    fn width(&self) -> u32 {
        self.size().w as u32
    }

    fn size(&self) -> Size<i32, BufferCoord> {
        match &self.0 {
            GlesTargetInternal::Image { dmabuf, .. } => dmabuf.size(),
            GlesTargetInternal::Surface { surface } => surface
                .get_size()
                .expect("a bound EGLSurface needs to have a size")
                .to_logical(1)
                .to_buffer(1, Transform::Normal),
            GlesTargetInternal::Texture { texture, .. } => texture.size(),
            GlesTargetInternal::Renderbuffer { buf, .. } => buf.size(),
        }
    }

    fn format(&self) -> Option<Fourcc> {
        let (gl_format, _) = self.0.format()?;
        gl_internal_format_to_fourcc(gl_format)
    }
}

impl GlesTargetInternal<'_> {
    fn format(&self) -> Option<(ffi::types::GLenum, bool)> {
        match self {
            GlesTargetInternal::Image { dmabuf, .. } => {
                let format = crate::backend::allocator::Buffer::format(*dmabuf).code;
                let has_alpha = has_alpha(format);
                let (format, _, _) = fourcc_to_gl_formats(format)?;

                Some((format, has_alpha))
            }
            GlesTargetInternal::Surface { surface, .. } => {
                let format = surface.pixel_format();
                let format = match (format.color_bits, format.alpha_bits) {
                    (24, 8) => ffi::RGB8,
                    (30, 2) => ffi::RGB10_A2,
                    (48, 16) => ffi::RGB16F,
                    _ => return None,
                };

                Some((format, true))
            }
            GlesTargetInternal::Texture { texture, .. } => Some((texture.0.format?, texture.0.has_alpha)),
            GlesTargetInternal::Renderbuffer { buf, .. } => Some((buf.0.format, buf.0.has_alpha)),
        }
    }

    #[profiling::function]
    fn make_current(&self, gl: &ffi::Gles2, egl: &EGLContext) -> Result<(), MakeCurrentError> {
        unsafe {
            if let GlesTargetInternal::Surface { surface, .. } = self {
                egl.make_current_with_surface(surface)?;
                gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
            } else {
                egl.make_current()?;
                match self {
                    GlesTargetInternal::Image { ref buf, .. } => {
                        gl.BindFramebuffer(ffi::FRAMEBUFFER, buf.fbo)
                    }
                    GlesTargetInternal::Texture { ref fbo, .. } => gl.BindFramebuffer(ffi::FRAMEBUFFER, *fbo),
                    GlesTargetInternal::Renderbuffer { ref fbo, .. } => {
                        gl.BindFramebuffer(ffi::FRAMEBUFFER, *fbo)
                    }
                    _ => unreachable!(),
                }
            }
            Ok(())
        }
    }
}

impl Drop for GlesTargetInternal<'_> {
    fn drop(&mut self) {
        match self {
            GlesTargetInternal::Texture {
                fbo,
                destruction_callback_sender,
                ..
            } => {
                let _ = destruction_callback_sender.send(CleanupResource::FramebufferObject(*fbo));
            }
            GlesTargetInternal::Renderbuffer { buf, fbo, .. } => {
                let _ = buf
                    .0
                    .destruction_callback_sender
                    .send(CleanupResource::FramebufferObject(*fbo));
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Capabilities of the [`GlesRenderer`]
pub enum Capability {
    /// GlesRenderer supports Instancing for render optimizations
    Instancing,
    /// GlesRenderer supports blitting between framebuffers
    Blit,
    /// GlesRenderer supports 10 bit formats
    _10Bit,
    /// GlesRenderer supports creating of Renderbuffers with usable formats
    Renderbuffer,
    /// GlesRenderer supports fencing
    Fencing,
    /// GlesRenderer supports fencing and exporting it to EGL
    ExportFence,
    /// GlesRenderer supports GL debug
    Debug,
}

/// A renderer utilizing OpenGL ES
pub struct GlesRenderer {
    // state
    min_filter: TextureFilter,
    max_filter: TextureFilter,
    debug_flags: DebugFlags,

    // internals
    egl: EGLContext,
    #[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
    egl_reader: Option<EGLBufferReader>,
    gl: ffi::Gles2,

    // optionals
    gl_version: GlVersion,
    pub(crate) extensions: Vec<String>,
    capabilities: Vec<Capability>,

    // shaders
    tex_program: GlesTexProgram,
    solid_program: GlesSolidProgram,

    // caches
    buffers: Vec<GlesBuffer>,
    dmabuf_cache: HashMap<WeakDmabuf, GlesTexture>,
    vbos: [ffi::types::GLuint; 2],
    vertices: Vec<f32>,
    non_opaque_damage: Vec<Rectangle<i32, Physical>>,
    opaque_damage: Vec<Rectangle<i32, Physical>>,

    // cleanup
    destruction_callback: Receiver<CleanupResource>,
    destruction_callback_sender: Sender<CleanupResource>,

    // markers
    _not_send: PhantomData<*mut ()>,

    // debug
    span: tracing::Span,
    gl_debug_span: Option<*mut tracing::Span>,
}

/// Handle to the currently rendered frame during [`GlesRenderer::render`](Renderer::render).
///
/// Leaking this frame will cause a variety of problems:
/// - It might prevent the frame from synchronizing the rendered framebuffer causing glitches.
/// - Depending on the bound target this can deadlock, if the same target is used later in any way.
/// - Additionally parts of the GL state might not be reset correctly, causing unexpected results for later render commands.
/// - The internal GL context and framebuffer will remain valid, no re-creation will be necessary.
pub struct GlesFrame<'frame, 'buffer> {
    renderer: &'frame mut GlesRenderer,
    target: &'frame mut GlesTarget<'buffer>,
    current_projection: Matrix3<f32>,
    transform: Transform,
    size: Size<i32, Physical>,
    tex_program_override: Option<(GlesTexProgram, Vec<Uniform<'static>>)>,
    finished: AtomicBool,

    span: EnteredSpan,
}

impl fmt::Debug for GlesFrame<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlesFrame")
            .field("renderer", &self.renderer)
            .field("target", &self.target)
            .field("current_projection", &self.current_projection)
            .field("transform", &self.transform)
            .field("tex_program_override", &self.tex_program_override)
            .field("size", &self.size)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for GlesRenderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlesRenderer")
            .field("buffers", &self.buffers)
            .field("extensions", &self.extensions)
            .field("capabilities", &self.capabilities)
            .field("tex_program", &self.tex_program)
            .field("solid_program", &self.solid_program)
            .field("dmabuf_cache", &self.dmabuf_cache)
            .field("egl", &self.egl)
            .field("gl_version", &self.gl_version)
            // ffi::Gles does not implement Debug
            .field("vbos", &self.vbos)
            .field("min_filter", &self.min_filter)
            .field("max_filter", &self.max_filter)
            .finish()
    }
}

extern "system" fn gl_debug_log(
    _source: ffi::types::GLenum,
    gltype: ffi::types::GLenum,
    _id: ffi::types::GLuint,
    _severity: ffi::types::GLenum,
    _length: ffi::types::GLsizei,
    message: *const ffi::types::GLchar,
    user_param: *mut std::ffi::c_void,
) {
    let _ = std::panic::catch_unwind(move || unsafe {
        let span = &mut *(user_param as *mut tracing::Span);
        let _guard = span.enter();
        let msg = CStr::from_ptr(message);
        let message_utf8 = msg.to_string_lossy();
        match gltype {
            ffi::DEBUG_TYPE_ERROR | ffi::DEBUG_TYPE_UNDEFINED_BEHAVIOR => {
                error!("[GL] {}", message_utf8)
            }
            ffi::DEBUG_TYPE_DEPRECATED_BEHAVIOR => warn!("[GL] {}", message_utf8),
            _ => debug!("[GL] {}", message_utf8),
        };
    });
}

impl GlesRenderer {
    /// Get the supported [`Capabilities`](Capability) of the renderer
    ///
    /// # Safety
    ///
    /// This operation will cause undefined behavior if the given EGLContext is active in another thread.
    pub unsafe fn supported_capabilities(context: &EGLContext) -> Result<Vec<Capability>, GlesError> {
        context.make_current()?;

        let gl = ffi::Gles2::load_with(|s| crate::backend::egl::get_proc_address(s) as *const _);
        let ext_ptr = gl.GetString(ffi::EXTENSIONS) as *const c_char;
        if ext_ptr.is_null() {
            return Err(GlesError::GLFunctionLoaderError);
        }

        let exts = {
            let p = CStr::from_ptr(ext_ptr);
            let list = String::from_utf8(p.to_bytes().to_vec()).unwrap_or_else(|_| String::new());
            list.split(' ').map(|e| e.to_string()).collect::<Vec<_>>()
        };

        let gl_version = version::GlVersion::try_from(&gl).unwrap_or_else(|_| {
            warn!("Failed to detect GLES version, defaulting to 2.0");
            version::GLES_2_0
        });

        let mut capabilities = Vec::new();
        // required for more optimized rendering, otherwise we render in batches
        if gl_version >= version::GLES_3_0
            || (exts.iter().any(|ext| ext == "GL_EXT_instanced_arrays")
                && exts.iter().any(|ext| ext == "GL_EXT_draw_instanced"))
        {
            capabilities.push(Capability::Instancing);
            debug!("Instancing is supported");
        }
        // required to use 8-bit color formats in renderbuffers, we don't deal with anything lower as a render target
        if gl_version >= version::GLES_3_0 || exts.iter().any(|ext| ext == "GL_OES_rgb8_rgba8") {
            capabilities.push(Capability::Renderbuffer);
            debug!("Rgba8 Renderbuffers are supported");
        }
        // required for blit operations
        if gl_version >= version::GLES_3_0 {
            capabilities.push(Capability::Blit);
            debug!("Blitting is supported");
            capabilities.push(Capability::_10Bit);
            debug!("10-bit formats are supported");
            capabilities.push(Capability::Fencing);
            debug!("Fencing is supported");
        }

        if exts.iter().any(|ext| ext == "GL_OES_EGL_sync") {
            debug!("EGL Fencing is supported");
            capabilities.push(Capability::ExportFence);
        }

        if exts.iter().any(|ext| ext == "GL_KHR_debug") {
            capabilities.push(Capability::Debug);
            debug!("GL Debug is supported");
        }

        Ok(capabilities)
    }

    /// Creates a new OpenGL ES renderer from a given [`EGLContext`]
    /// with all [`supported capabilities`](Self::supported_capabilities).
    ///
    /// # Safety
    ///
    /// This operation will cause undefined behavior if the given EGLContext is active in another thread.
    ///
    /// See: [`with_capabilities`](Self::with_capabilities) for more information
    pub unsafe fn new(context: EGLContext) -> Result<GlesRenderer, GlesError> {
        let supported_capabilities = Self::supported_capabilities(&context)?;
        Self::with_capabilities(context, supported_capabilities)
    }

    /// Creates a new OpenGL ES renderer from a given [`EGLContext`]
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
    /// - Shm buffers can be released after a successful import, without the texture handle becoming invalid.
    /// - Texture filtering starts with Linear-downscaling and Linear-upscaling.
    /// - If OpenGL ES 3.0 is not available and the underlying [`EGLContext`] is shared, memory textures
    ///   will insert `glFinish`-calls into the pipeline. Consider not sharing contexts, if OpenGL ES 3 isn't available.
    pub unsafe fn with_capabilities(
        context: EGLContext,
        capabilities: impl IntoIterator<Item = Capability>,
    ) -> Result<GlesRenderer, GlesError> {
        let span = info_span!(parent: &context.span, "renderer_gles2");
        let _guard = span.enter();

        context.make_current()?;

        let supported_capabilities = Self::supported_capabilities(&context)?;
        let requested_capabilities = capabilities.into_iter().collect::<Vec<_>>();

        let unsupported_capabilities = requested_capabilities
            .iter()
            .copied()
            .filter(|c| !supported_capabilities.contains(c))
            .collect::<Vec<_>>();

        if let Some(missing_capability) = unsupported_capabilities.first() {
            let err = match missing_capability {
                Capability::Instancing => {
                    GlesError::GLExtensionNotSupported(&["GL_EXT_instanced_arrays", "GL_EXT_draw_instanced"])
                }
                Capability::Blit | Capability::_10Bit | Capability::Fencing => {
                    GlesError::GLVersionNotSupported(version::GLES_3_0)
                }
                Capability::Renderbuffer => GlesError::GLExtensionNotSupported(&["GL_OES_rgb8_rgba8"]),
                Capability::ExportFence => GlesError::GLExtensionNotSupported(&["GL_OES_EGL_sync"]),
                Capability::Debug => GlesError::GLExtensionNotSupported(&["GL_KHR_debug"]),
            };
            return Err(err);
        };

        let (gl, gl_version, exts, capabilities, gl_debug_span) = {
            let gl = ffi::Gles2::load_with(|s| crate::backend::egl::get_proc_address(s) as *const _);
            let ext_ptr = gl.GetString(ffi::EXTENSIONS) as *const c_char;
            if ext_ptr.is_null() {
                return Err(GlesError::GLFunctionLoaderError);
            }

            let exts = {
                let p = CStr::from_ptr(ext_ptr);
                let list = String::from_utf8(p.to_bytes().to_vec()).unwrap_or_else(|_| String::new());
                list.split(' ').map(|e| e.to_string()).collect::<Vec<_>>()
            };

            info!("Initializing OpenGL ES Renderer");
            info!(
                "GL Version: {:?}",
                CStr::from_ptr(gl.GetString(ffi::VERSION) as *const c_char)
            );
            info!(
                "GL Vendor: {:?}",
                CStr::from_ptr(gl.GetString(ffi::VENDOR) as *const c_char)
            );
            info!(
                "GL Renderer: {:?}",
                CStr::from_ptr(gl.GetString(ffi::RENDERER) as *const c_char)
            );
            info!("Supported GL Extensions: {:?}", exts);

            let gl_version = version::GlVersion::try_from(&gl).unwrap_or_else(|_| {
                warn!("Failed to detect GLES version, defaulting to 2.0");
                version::GLES_2_0
            });

            // required for the manditory wl_shm formats
            if !exts.iter().any(|ext| ext == "GL_EXT_texture_format_BGRA8888") {
                return Err(GlesError::GLExtensionNotSupported(&[
                    "GL_EXT_texture_format_BGRA8888",
                ]));
            }

            // required for buffers without linear memory layout
            if gl_version < version::GLES_3_0 && !exts.iter().any(|ext| ext == "GL_EXT_unpack_subimage") {
                return Err(GlesError::GLExtensionNotSupported(&["GL_EXT_unpack_subimage"]));
            }

            let gl_debug_span = if requested_capabilities.contains(&Capability::Debug) {
                gl.Enable(ffi::DEBUG_OUTPUT);
                gl.Enable(ffi::DEBUG_OUTPUT_SYNCHRONOUS);
                let span = Box::into_raw(Box::new(span.clone()));
                gl.DebugMessageCallback(Some(gl_debug_log), span as *mut _);
                Some(span)
            } else {
                None
            };

            (gl, gl_version, exts, requested_capabilities, gl_debug_span)
        };

        let (tx, rx) = channel();
        let tex_program = texture_program(&gl, shaders::FRAGMENT_SHADER, &[], tx.clone())?;
        let solid_program = solid_program(&gl)?;

        // Initialize vertices based on drawing methodology.
        let vertices: &[ffi::types::GLfloat] = if capabilities.contains(&Capability::Instancing) {
            &INSTANCED_VERTS
        } else {
            &TRIANGLE_VERTS
        };

        let mut vbos = [0; 2];
        gl.GenBuffers(vbos.len() as i32, vbos.as_mut_ptr());
        gl.BindBuffer(ffi::ARRAY_BUFFER, vbos[0]);
        gl.BufferData(
            ffi::ARRAY_BUFFER,
            std::mem::size_of_val(vertices) as isize,
            vertices.as_ptr() as *const _,
            ffi::STATIC_DRAW,
        );
        gl.BindBuffer(ffi::ARRAY_BUFFER, vbos[1]);
        gl.BufferData(
            ffi::ARRAY_BUFFER,
            (std::mem::size_of::<ffi::types::GLfloat>() * OUTPUT_VERTS.len()) as isize,
            OUTPUT_VERTS.as_ptr() as *const _,
            ffi::STATIC_DRAW,
        );
        gl.BindBuffer(ffi::ARRAY_BUFFER, 0);

        context
            .user_data()
            .insert_if_missing_threadsafe(ContextId::<GlesTexture>::new);

        drop(_guard);

        let renderer = GlesRenderer {
            gl,
            egl: context,
            #[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
            egl_reader: None,

            extensions: exts,
            gl_version,
            capabilities,

            tex_program,
            solid_program,
            vbos,
            min_filter: TextureFilter::Linear,
            max_filter: TextureFilter::Linear,

            buffers: Vec::new(),
            dmabuf_cache: std::collections::HashMap::new(),
            vertices: Vec::with_capacity(6 * 16),
            non_opaque_damage: Vec::with_capacity(16),
            opaque_damage: Vec::with_capacity(16),

            destruction_callback: rx,
            destruction_callback_sender: tx,

            debug_flags: DebugFlags::empty(),
            _not_send: PhantomData,
            span,
            gl_debug_span,
        };
        renderer.egl.unbind()?;
        Ok(renderer)
    }

    fn bind_texture<'a>(&mut self, texture: &'a GlesTexture) -> Result<GlesTarget<'a>, GlesError> {
        unsafe {
            self.egl.make_current()?;
        }

        let bind = || {
            let mut sync_lock = texture.0.sync.write().unwrap();
            let mut fbo = 0;
            unsafe {
                sync_lock.wait_for_all(&self.gl);
                self.gl.GenFramebuffers(1, &mut fbo as *mut _);
                self.gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo);
                self.gl.FramebufferTexture2D(
                    ffi::FRAMEBUFFER,
                    ffi::COLOR_ATTACHMENT0,
                    ffi::TEXTURE_2D,
                    texture.0.texture,
                    0,
                );
                let status = self.gl.CheckFramebufferStatus(ffi::FRAMEBUFFER);
                self.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);

                if status != ffi::FRAMEBUFFER_COMPLETE {
                    self.gl.DeleteFramebuffers(1, &mut fbo as *mut _);
                    return Err(GlesError::FramebufferBindingError);
                }
            }

            Ok(GlesTarget(GlesTargetInternal::Texture {
                texture: texture.clone(),
                sync_lock,
                destruction_callback_sender: self.destruction_callback_sender.clone(),
                fbo,
            }))
        };

        bind().inspect_err(|_| {
            if let Err(err) = self.unbind() {
                self.span.in_scope(|| warn!(?err, "Failed to unbind on err"));
            }
        })
    }

    #[profiling::function]
    fn unbind(&mut self) -> Result<(), GlesError> {
        unsafe {
            self.egl.make_current()?;
        }
        unsafe { self.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0) };
        self.cleanup();
        self.egl.unbind()?;
        Ok(())
    }

    #[profiling::function]
    fn cleanup(&mut self) {
        self.dmabuf_cache.retain(|entry, _tex| !entry.is_gone());
        // Free outdated buffer resources
        // TODO: Replace with `drain_filter` once it lands
        let mut i = 0;
        while i != self.buffers.len() {
            if self.buffers[i].dmabuf.is_gone() {
                let old = self.buffers.remove(i);
                unsafe {
                    self.gl.DeleteFramebuffers(1, &old.fbo as *const _);
                    self.gl.DeleteRenderbuffers(1, &old.rbo as *const _);
                    ffi_egl::DestroyImageKHR(**self.egl.display().get_display_handle(), old.image);
                }
            } else {
                i += 1;
            }
        }
        for resource in self.destruction_callback.try_iter() {
            match resource {
                CleanupResource::Texture(texture) => unsafe {
                    self.gl.DeleteTextures(1, &texture);
                },
                CleanupResource::EGLImage(image) => unsafe {
                    ffi_egl::DestroyImageKHR(**self.egl.display().get_display_handle(), image);
                },
                CleanupResource::FramebufferObject(fbo) => unsafe {
                    self.gl.DeleteFramebuffers(1, &fbo);
                },
                CleanupResource::RenderbufferObject(rbo) => unsafe {
                    self.gl.DeleteRenderbuffers(1, &rbo);
                },
                CleanupResource::Mapping(pbo, mapping) => unsafe {
                    if !mapping.is_null() {
                        self.gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, pbo);
                        self.gl.UnmapBuffer(ffi::PIXEL_PACK_BUFFER);
                        self.gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, 0);
                    }
                    self.gl.DeleteBuffers(1, &pbo);
                },
                CleanupResource::Program(program) => unsafe {
                    self.gl.DeleteProgram(program);
                },
                CleanupResource::Sync(sync) => unsafe {
                    self.gl.DeleteSync(sync);
                },
            }
        }
    }

    /// Returns the supported [`Capabilities`](Capability) of this renderer.
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportMemWl for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&crate::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, BufferCoord>],
    ) -> Result<GlesTexture, GlesError> {
        use crate::wayland::shm::with_buffer_contents;

        // why not store a `GlesTexture`? because the user might do so.
        // this is guaranteed a non-public internal type, so we are good.
        type CacheMap = HashMap<ContextId<GlesTexture>, Arc<GlesTextureInternal>>;

        let mut surface_lock = surface.as_ref().map(|surface_data| {
            surface_data
                .data_map
                .get_or_insert_threadsafe(|| Arc::new(Mutex::new(CacheMap::new())))
                .lock()
                .unwrap()
        });

        with_buffer_contents(buffer, |ptr, len, data| {
            let offset = data.offset;
            let width = data.width;
            let height = data.height;
            let stride = data.stride;
            let fourcc =
                shm_format_to_fourcc(data.format).ok_or(GlesError::UnsupportedWlPixelFormat(data.format))?;

            if self.gl_version.major >= 3 {
                if !SUPPORTED_MEM_FORMATS_3.contains(&fourcc) {
                    return Err(GlesError::UnsupportedWlPixelFormat(data.format));
                }
            } else if !SUPPORTED_MEM_FORMATS_2.contains(&fourcc) {
                return Err(GlesError::UnsupportedWlPixelFormat(data.format));
            }

            let has_alpha = has_alpha(fourcc);
            let (mut internal_format, read_format, type_) =
                fourcc_to_gl_formats(fourcc).ok_or(GlesError::UnsupportedWlPixelFormat(data.format))?;
            if self.gl_version.major == 2 {
                // es 2.0 doesn't define sized variants
                internal_format = match internal_format {
                    ffi::BGRA_EXT => ffi::BGRA_EXT,
                    ffi::RGBA8 => ffi::RGBA,
                    ffi::RGB8 => ffi::RGB,
                    _ => unreachable!(),
                };
            }

            // number of bytes per pixel
            let pixelsize = gl_bpp(read_format, type_).expect("We check the format before") / 8;
            // ensure consistency, the SHM handler of smithay should ensure this
            assert!((offset + (height - 1) * stride + width * pixelsize as i32) as usize <= len);

            let mut upload_full = false;

            let id = self.context_id();
            let texture = GlesTexture(
                surface_lock
                    .as_ref()
                    .and_then(|cache| cache.get(&id).cloned())
                    .filter(|texture| texture.size == (width, height).into())
                    .unwrap_or_else(|| {
                        let mut tex = 0;
                        unsafe { self.gl.GenTextures(1, &mut tex) };
                        // new texture, upload in full
                        upload_full = true;
                        let new = Arc::new(GlesTextureInternal {
                            texture: tex,
                            sync: RwLock::default(),
                            format: Some(internal_format),
                            has_alpha,
                            is_external: false,
                            y_inverted: false,
                            size: (width, height).into(),
                            egl_images: None,
                            destruction_callback_sender: self.destruction_callback_sender.clone(),
                        });
                        if let Some(cache) = surface_lock.as_mut() {
                            cache.insert(id, new.clone());
                        }
                        new
                    }),
            );

            let mut sync_lock = texture.0.sync.write().unwrap();
            unsafe {
                self.egl.make_current()?;
                sync_lock.wait_for_all(&self.gl);
                self.gl.BindTexture(ffi::TEXTURE_2D, texture.0.texture);
                self.gl
                    .TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
                self.gl
                    .TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
                self.gl
                    .PixelStorei(ffi::UNPACK_ROW_LENGTH, stride / pixelsize as i32);

                if upload_full || damage.is_empty() {
                    trace!("Uploading shm texture");
                    self.gl.TexImage2D(
                        ffi::TEXTURE_2D,
                        0,
                        internal_format as i32,
                        width,
                        height,
                        0,
                        read_format,
                        type_,
                        ptr.offset(offset as isize) as *const _,
                    );
                } else {
                    for region in damage.iter() {
                        trace!("Uploading partial shm texture");
                        self.gl.PixelStorei(ffi::UNPACK_SKIP_PIXELS, region.loc.x);
                        self.gl.PixelStorei(ffi::UNPACK_SKIP_ROWS, region.loc.y);
                        self.gl.TexSubImage2D(
                            ffi::TEXTURE_2D,
                            0,
                            region.loc.x,
                            region.loc.y,
                            region.size.w,
                            region.size.h,
                            read_format,
                            type_,
                            ptr.offset(offset as isize) as *const _,
                        );
                        self.gl.PixelStorei(ffi::UNPACK_SKIP_PIXELS, 0);
                        self.gl.PixelStorei(ffi::UNPACK_SKIP_ROWS, 0);
                    }
                }

                self.gl.PixelStorei(ffi::UNPACK_ROW_LENGTH, 0);
                self.gl.BindTexture(ffi::TEXTURE_2D, 0);

                if self.capabilities.contains(&Capability::Fencing) {
                    sync_lock.update_write(&self.gl);
                } else if self.egl.is_shared() {
                    self.gl.Finish();
                }
            }
            std::mem::drop(sync_lock);

            Ok(texture)
        })
        .map_err(GlesError::BufferAccessError)?
    }
}

const SUPPORTED_MEM_FORMATS_2: &[Fourcc] = &[
    Fourcc::Abgr8888,
    Fourcc::Xbgr8888,
    Fourcc::Argb8888,
    Fourcc::Xrgb8888,
];
const SUPPORTED_MEM_FORMATS_3: &[Fourcc] = &[
    Fourcc::Abgr8888,
    Fourcc::Xbgr8888,
    Fourcc::Argb8888,
    Fourcc::Xrgb8888,
    Fourcc::Abgr2101010,
    Fourcc::Xbgr2101010,
    Fourcc::Abgr16161616f,
    Fourcc::Xbgr16161616f,
];

impl ImportMem for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn import_memory(
        &mut self,
        data: &[u8],
        format: Fourcc,
        size: Size<i32, BufferCoord>,
        flipped: bool,
    ) -> Result<GlesTexture, GlesError> {
        if data.len()
            < (size.w * size.h) as usize
                * (get_bpp(format).ok_or(GlesError::UnsupportedPixelFormat(format))? / 8)
        {
            return Err(GlesError::UnexpectedSize);
        }

        if self.gl_version.major >= 3 {
            if !SUPPORTED_MEM_FORMATS_3.contains(&format) {
                return Err(GlesError::UnsupportedPixelFormat(format));
            }
        } else if !SUPPORTED_MEM_FORMATS_2.contains(&format) {
            return Err(GlesError::UnsupportedPixelFormat(format));
        }

        let has_alpha = has_alpha(format);
        let (mut internal, format, layout) =
            fourcc_to_gl_formats(format).expect("We check the format before");
        if self.gl_version.major == 2 {
            // es 2.0 doesn't define sized variants
            internal = match internal {
                ffi::RGBA8 => ffi::RGBA,
                ffi::RGB8 => ffi::RGB,
                ffi::BGRA_EXT => ffi::BGRA_EXT,
                _ => unreachable!(),
            };
        }

        let texture = GlesTexture(Arc::new({
            let mut tex = 0;
            unsafe {
                self.egl.make_current()?;
                self.gl.GenTextures(1, &mut tex);
                self.gl.BindTexture(ffi::TEXTURE_2D, tex);
                self.gl
                    .TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
                self.gl
                    .TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
                self.gl.TexImage2D(
                    ffi::TEXTURE_2D,
                    0,
                    internal as i32,
                    size.w,
                    size.h,
                    0,
                    format,
                    layout,
                    data.as_ptr() as *const _,
                );
                self.gl.BindTexture(ffi::TEXTURE_2D, 0);
            }

            let mut sync = RwLock::<TextureSync>::default();
            if self.capabilities.contains(&Capability::Fencing) {
                sync.get_mut().unwrap().update_write(&self.gl);
            } else if self.egl.is_shared() {
                unsafe {
                    self.gl.Finish();
                }
            };

            // new texture, upload in full
            GlesTextureInternal {
                texture: tex,
                sync,
                format: Some(internal),
                has_alpha,
                is_external: false,
                y_inverted: flipped,
                size,
                egl_images: None,
                destruction_callback_sender: self.destruction_callback_sender.clone(),
            }
        }));

        Ok(texture)
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn update_memory(
        &mut self,
        texture: &Self::TextureId,
        data: &[u8],
        region: Rectangle<i32, BufferCoord>,
    ) -> Result<(), Self::Error> {
        if texture.0.format.is_none() {
            return Err(GlesError::UnknownPixelFormat);
        }
        if texture.0.is_external {
            return Err(GlesError::UnsupportedPixelLayout);
        }
        let (read_format, type_) = gl_read_for_internal(texture.0.format.expect("We check that before"))
            .ok_or(GlesError::UnknownPixelFormat)?;

        if data.len()
            < (region.size.w * region.size.h) as usize
                * (gl_bpp(read_format, type_).ok_or(GlesError::UnknownPixelFormat)? / 8)
        {
            return Err(GlesError::UnexpectedSize);
        }

        let mut sync_lock = texture.0.sync.write().unwrap();
        unsafe {
            self.egl.make_current()?;
            sync_lock.wait_for_all(&self.gl);
            self.gl.BindTexture(ffi::TEXTURE_2D, texture.0.texture);
            self.gl
                .TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
            self.gl
                .TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
            self.gl.PixelStorei(ffi::UNPACK_ROW_LENGTH, texture.0.size.w);
            self.gl.PixelStorei(ffi::UNPACK_SKIP_PIXELS, region.loc.x);
            self.gl.PixelStorei(ffi::UNPACK_SKIP_ROWS, region.loc.y);
            self.gl.TexSubImage2D(
                ffi::TEXTURE_2D,
                0,
                region.loc.x,
                region.loc.y,
                region.size.w,
                region.size.h,
                read_format,
                type_,
                data.as_ptr() as *const _,
            );
            self.gl.PixelStorei(ffi::UNPACK_ROW_LENGTH, 0);
            self.gl.PixelStorei(ffi::UNPACK_SKIP_PIXELS, 0);
            self.gl.PixelStorei(ffi::UNPACK_SKIP_ROWS, 0);
            self.gl.BindTexture(ffi::TEXTURE_2D, 0);

            if self.capabilities.contains(&Capability::Fencing) {
                sync_lock.update_write(&self.gl);
            } else if self.egl.is_shared() {
                self.gl.Finish();
            }
        }

        Ok(())
    }

    fn mem_formats(&self) -> Box<dyn Iterator<Item = Fourcc>> {
        if self.gl_version.major >= 3 {
            Box::new(SUPPORTED_MEM_FORMATS_3.iter().copied())
        } else {
            Box::new(SUPPORTED_MEM_FORMATS_2.iter().copied())
        }
    }
}

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
impl ImportEgl for GlesRenderer {
    fn bind_wl_display(
        &mut self,
        display: &wayland_server::DisplayHandle,
    ) -> Result<(), crate::backend::egl::Error> {
        self.egl_reader = Some(self.egl.display().bind_wl_display(display)?);
        Ok(())
    }

    fn unbind_wl_display(&mut self) {
        self.egl_reader = None;
    }

    fn egl_reader(&self) -> Option<&EGLBufferReader> {
        self.egl_reader.as_ref()
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn import_egl_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        _surface: Option<&crate::wayland::compositor::SurfaceData>,
        _damage: &[Rectangle<i32, BufferCoord>],
    ) -> Result<GlesTexture, GlesError> {
        if !self.extensions.iter().any(|ext| ext == "GL_OES_EGL_image") {
            return Err(GlesError::GLExtensionNotSupported(&["GL_OES_EGL_image"]));
        }

        if self.egl_reader().is_none() {
            return Err(GlesError::EGLBufferAccessError(
                crate::backend::egl::BufferAccessError::NotManaged(crate::backend::egl::EGLError::BadDisplay),
            ));
        }

        // We can not use the caching logic for textures here as the
        // egl buffers a potentially managed external which will fail the
        // clean up check if the buffer is still alive. For wl_drm the
        // is_alive check will always return true and the cache entry
        // will never be cleaned up.
        let egl = self
            .egl_reader
            .as_ref()
            .unwrap()
            .egl_buffer_contents(buffer)
            .map_err(GlesError::EGLBufferAccessError)?;

        let tex = self.import_egl_image(egl.image(0).unwrap(), egl.format == EGLFormat::External, None)?;

        let texture = GlesTexture(Arc::new(GlesTextureInternal {
            texture: tex,
            sync: RwLock::default(),
            format: match egl.format {
                EGLFormat::RGB | EGLFormat::RGBA => Some(ffi::RGBA8),
                EGLFormat::External => None,
                _ => unreachable!("EGLBuffer currenly does not expose multi-planar buffers to us"),
            },
            has_alpha: !matches!(egl.format, EGLFormat::RGB),
            is_external: egl.format == EGLFormat::External,
            y_inverted: egl.y_inverted,
            size: egl.size,
            egl_images: Some(egl.into_images()),
            destruction_callback_sender: self.destruction_callback_sender.clone(),
        }));

        Ok(texture)
    }
}

impl ImportDma for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn import_dmabuf(
        &mut self,
        buffer: &Dmabuf,
        _damage: Option<&[Rectangle<i32, BufferCoord>]>,
    ) -> Result<GlesTexture, GlesError> {
        use crate::backend::allocator::Buffer;
        if !self.extensions.iter().any(|ext| ext == "GL_OES_EGL_image") {
            return Err(GlesError::GLExtensionNotSupported(&["GL_OES_EGL_image"]));
        }

        self.existing_dmabuf_texture(buffer)?.map(Ok).unwrap_or_else(|| {
            let is_external = !self.egl.dmabuf_render_formats().contains(&buffer.format());
            let image = self
                .egl
                .display()
                .create_image_from_dmabuf(buffer)
                .map_err(GlesError::BindBufferEGLError)?;

            let tex = self.import_egl_image(image, is_external, None)?;
            let format = fourcc_to_gl_formats(buffer.format().code)
                .map(|(internal, _, _)| internal)
                .unwrap_or(ffi::RGBA8);
            let has_alpha = has_alpha(buffer.format().code);
            let texture = GlesTexture(Arc::new(GlesTextureInternal {
                texture: tex,
                sync: RwLock::default(),
                format: Some(format),
                has_alpha,
                is_external,
                y_inverted: buffer.y_inverted(),
                size: buffer.size(),
                egl_images: Some(vec![image]),
                destruction_callback_sender: self.destruction_callback_sender.clone(),
            }));
            self.dmabuf_cache.insert(buffer.weak(), texture.clone());
            Ok(texture)
        })
    }

    fn dmabuf_formats(&self) -> FormatSet {
        self.egl.dmabuf_texture_formats().clone()
    }

    fn has_dmabuf_format(&self, format: Format) -> bool {
        self.egl.dmabuf_texture_formats().contains(&format)
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportDmaWl for GlesRenderer {}

impl GlesRenderer {
    #[profiling::function]
    fn existing_dmabuf_texture(&self, buffer: &Dmabuf) -> Result<Option<GlesTexture>, GlesError> {
        let Some(texture) = self.dmabuf_cache.get(&buffer.weak()) else {
            return Ok(None);
        };

        trace!("Re-using texture {:?} for {:?}", texture.0.texture, buffer);
        if let Some(egl_images) = texture.0.egl_images.as_ref() {
            if egl_images[0] == ffi_egl::NO_IMAGE_KHR {
                return Ok(None);
            }
            let tex = Some(texture.0.texture);
            self.import_egl_image(egl_images[0], texture.0.is_external, tex)?;
        }
        Ok(Some(texture.clone()))
    }

    #[profiling::function]
    fn import_egl_image(
        &self,
        image: EGLImage,
        is_external: bool,
        tex: Option<u32>,
    ) -> Result<u32, GlesError> {
        unsafe {
            self.egl.make_current()?;
        }
        let tex = tex.unwrap_or_else(|| unsafe {
            let mut tex = 0;
            self.gl.GenTextures(1, &mut tex);
            tex
        });
        let target = if is_external {
            ffi::TEXTURE_EXTERNAL_OES
        } else {
            ffi::TEXTURE_2D
        };
        unsafe {
            self.gl.BindTexture(target, tex);
            self.gl.EGLImageTargetTexture2DOES(target, image);
            self.gl.BindTexture(target, 0);
        }

        Ok(tex)
    }
}

impl ExportMem for GlesRenderer {
    type TextureMapping = GlesMapping;

    #[instrument(level = "trace", parent = &self.span, skip(self, target))]
    #[profiling::function]
    fn copy_framebuffer(
        &mut self,
        target: &GlesTarget<'_>,
        region: Rectangle<i32, BufferCoord>,
        fourcc: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        target.0.make_current(&self.gl, &self.egl)?;

        let (_, has_alpha) = target.0.format().ok_or(GlesError::UnknownPixelFormat)?;
        let (_, format, layout) = fourcc_to_gl_formats(fourcc).ok_or(GlesError::UnknownPixelFormat)?;

        let mut pbo = 0;
        let err = unsafe {
            self.gl.GetError(); // clear errors
            self.gl.GenBuffers(1, &mut pbo);
            self.gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, pbo);
            let bpp = gl_bpp(format, layout).ok_or(GlesError::UnsupportedPixelLayout)? / 8;
            let size = (region.size.w * region.size.h * bpp as i32) as isize;
            self.gl
                .BufferData(ffi::PIXEL_PACK_BUFFER, size, ptr::null(), ffi::STREAM_READ);
            self.gl
                .ReadBuffer(if matches!(target.0, GlesTargetInternal::Surface { .. }) {
                    ffi::BACK
                } else {
                    ffi::COLOR_ATTACHMENT0
                });
            self.gl.ReadPixels(
                region.loc.x,
                region.loc.y,
                region.size.w,
                region.size.h,
                format,
                layout,
                ptr::null_mut(),
            );
            self.gl.ReadBuffer(ffi::NONE);
            self.gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, 0);
            self.gl.GetError()
        };

        match err {
            ffi::NO_ERROR => Ok(GlesMapping {
                pbo,
                format,
                layout,
                has_alpha,
                size: region.size,
                mapping: AtomicPtr::new(ptr::null_mut()),
                destruction_callback_sender: self.destruction_callback_sender.clone(),
            }),
            ffi::INVALID_ENUM | ffi::INVALID_OPERATION => Err(GlesError::UnsupportedPixelFormat(fourcc)),
            _ => Err(GlesError::UnknownPixelFormat),
        }
    }

    fn can_read_texture(&mut self, texture: &Self::TextureId) -> Result<bool, GlesError> {
        // if we can't bind the texture, we can't read it
        Ok(self.bind_texture(texture).is_ok())
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, BufferCoord>,
        fourcc: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        let mut pbo = 0;
        let target = self.bind_texture(texture)?;
        target.0.make_current(&self.gl, &self.egl)?;

        let (_, format, layout) = fourcc_to_gl_formats(fourcc).ok_or(GlesError::UnknownPixelFormat)?;
        let bpp = gl_bpp(format, layout).expect("We check the format before") / 8;

        let err = unsafe {
            self.gl.GetError(); // clear errors
            self.gl.GenBuffers(1, &mut pbo);
            self.gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, pbo);
            self.gl.BufferData(
                ffi::PIXEL_PACK_BUFFER,
                (region.size.w * region.size.h * bpp as i32) as isize,
                ptr::null(),
                ffi::STREAM_READ,
            );
            self.gl.ReadBuffer(ffi::COLOR_ATTACHMENT0);
            self.gl.ReadPixels(
                region.loc.x,
                region.loc.y,
                region.size.w,
                region.size.h,
                format,
                layout,
                ptr::null_mut(),
            );
            self.gl.ReadBuffer(ffi::NONE);
            self.gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, 0);
            self.gl.GetError()
        };

        match err {
            ffi::NO_ERROR => Ok(GlesMapping {
                pbo,
                format,
                layout,
                has_alpha: texture.0.has_alpha,
                size: region.size,
                mapping: AtomicPtr::new(ptr::null_mut()),
                destruction_callback_sender: self.destruction_callback_sender.clone(),
            }),
            ffi::INVALID_ENUM | ffi::INVALID_OPERATION => Err(GlesError::UnsupportedPixelFormat(fourcc)),
            _ => Err(GlesError::UnknownPixelFormat),
        }
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn map_texture<'a>(
        &mut self,
        texture_mapping: &'a Self::TextureMapping,
    ) -> Result<&'a [u8], Self::Error> {
        unsafe {
            self.egl.make_current()?;
        }

        let size = texture_mapping.size();
        let len = size.w * size.h * 4;

        let mapping_ptr = texture_mapping.mapping.load(Ordering::SeqCst);
        let ptr = if mapping_ptr.is_null() {
            unsafe {
                self.gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, texture_mapping.pbo);
                let ptr = self
                    .gl
                    .MapBufferRange(ffi::PIXEL_PACK_BUFFER, 0, len as isize, ffi::MAP_READ_BIT);
                self.gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, 0);

                if ptr.is_null() {
                    return Err(GlesError::MappingError);
                }

                texture_mapping.mapping.store(ptr, Ordering::SeqCst);
                ptr
            }
        } else {
            mapping_ptr
        };

        unsafe { Ok(slice::from_raw_parts(ptr as *const u8, len as usize)) }
    }
}

impl Bind<EGLSurface> for GlesRenderer {
    fn bind<'a>(&mut self, surface: &'a mut EGLSurface) -> Result<GlesTarget<'a>, GlesError> {
        Ok(GlesTarget(GlesTargetInternal::Surface { surface }))
    }
}

impl Bind<Dmabuf> for GlesRenderer {
    fn bind<'a>(&mut self, dmabuf: &'a mut Dmabuf) -> Result<GlesTarget<'a>, GlesError> {
        let mut bind = |dmabuf: &'a mut Dmabuf| {
            let buf = self
                .buffers
                .iter_mut()
                .find(|buffer| {
                    if let Some(dma) = buffer.dmabuf.upgrade() {
                        dma == *dmabuf
                    } else {
                        false
                    }
                })
                .map(|buf| Ok(buf.clone()))
                .unwrap_or_else(|| {
                    unsafe {
                        self.egl.make_current()?;
                    }

                    trace!("Creating EGLImage for Dmabuf: {:?}", dmabuf);
                    let image = self
                        .egl
                        .display()
                        .create_image_from_dmabuf(dmabuf)
                        .map_err(GlesError::BindBufferEGLError)?;

                    unsafe {
                        let mut rbo = 0;
                        self.gl.GenRenderbuffers(1, &mut rbo as *mut _);
                        self.gl.BindRenderbuffer(ffi::RENDERBUFFER, rbo);
                        self.gl
                            .EGLImageTargetRenderbufferStorageOES(ffi::RENDERBUFFER, image);
                        self.gl.BindRenderbuffer(ffi::RENDERBUFFER, 0);

                        let mut fbo = 0;
                        self.gl.GenFramebuffers(1, &mut fbo as *mut _);
                        self.gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo);
                        self.gl.FramebufferRenderbuffer(
                            ffi::FRAMEBUFFER,
                            ffi::COLOR_ATTACHMENT0,
                            ffi::RENDERBUFFER,
                            rbo,
                        );
                        let status = self.gl.CheckFramebufferStatus(ffi::FRAMEBUFFER);
                        self.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);

                        if status != ffi::FRAMEBUFFER_COMPLETE {
                            self.gl.DeleteFramebuffers(1, &mut fbo as *mut _);
                            self.gl.DeleteRenderbuffers(1, &mut rbo as *mut _);
                            ffi_egl::DestroyImageKHR(**self.egl.display().get_display_handle(), image);
                            return Err(GlesError::FramebufferBindingError);
                        }
                        let buf = GlesBuffer {
                            dmabuf: dmabuf.weak(),
                            image,
                            rbo,
                            fbo,
                        };

                        self.buffers.push(buf.clone());

                        Ok(buf)
                    }
                })?;

            Ok(GlesTarget(GlesTargetInternal::Image { buf, dmabuf }))
        };

        bind(dmabuf).inspect_err(|_| {
            if let Err(err) = self.unbind() {
                self.span.in_scope(|| warn!(?err, "Failed to unbind on err"));
            }
        })
    }

    fn supported_formats(&self) -> Option<FormatSet> {
        Some(self.egl.display().dmabuf_render_formats().clone())
    }
}

impl Bind<GlesTexture> for GlesRenderer {
    fn bind<'a>(&mut self, texture: &'a mut GlesTexture) -> Result<GlesTarget<'a>, GlesError> {
        self.bind_texture(texture)
    }
}

impl Bind<GlesRenderbuffer> for GlesRenderer {
    fn bind<'a>(&mut self, renderbuffer: &'a mut GlesRenderbuffer) -> Result<GlesTarget<'a>, GlesError> {
        unsafe {
            self.egl.make_current()?;
        }

        let bind = |renderbuffer: &'a mut GlesRenderbuffer| {
            let mut fbo = 0;
            unsafe {
                self.gl.GenFramebuffers(1, &mut fbo as *mut _);
                self.gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo);
                self.gl.BindRenderbuffer(ffi::RENDERBUFFER, renderbuffer.0.rbo);
                self.gl.FramebufferRenderbuffer(
                    ffi::FRAMEBUFFER,
                    ffi::COLOR_ATTACHMENT0,
                    ffi::RENDERBUFFER,
                    renderbuffer.0.rbo,
                );
                let status = self.gl.CheckFramebufferStatus(ffi::FRAMEBUFFER);
                self.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
                self.gl.BindRenderbuffer(ffi::RENDERBUFFER, 0);

                if status != ffi::FRAMEBUFFER_COMPLETE {
                    self.gl.DeleteFramebuffers(1, &mut fbo as *mut _);
                    return Err(GlesError::FramebufferBindingError);
                }
            }

            Ok(GlesTarget(GlesTargetInternal::Renderbuffer {
                buf: renderbuffer,
                fbo,
            }))
        };

        bind(renderbuffer).inspect_err(|_| {
            if let Err(err) = self.unbind() {
                self.span.in_scope(|| warn!(?err, "Failed to unbind on err"));
            }
        })
    }
}

impl Offscreen<GlesTexture> for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn create_buffer(
        &mut self,
        format: Fourcc,
        size: Size<i32, BufferCoord>,
    ) -> Result<GlesTexture, GlesError> {
        let has_alpha = has_alpha(format);
        let (internal, format, layout) =
            fourcc_to_gl_formats(format).ok_or(GlesError::UnsupportedPixelFormat(format))?;
        if (internal != ffi::RGBA8 && internal != ffi::BGRA_EXT)
            && !self.capabilities.contains(&Capability::_10Bit)
        {
            return Err(GlesError::UnsupportedPixelLayout);
        }

        let tex = unsafe {
            self.egl.make_current()?;
            let mut tex = 0;
            self.gl.GenTextures(1, &mut tex);
            self.gl.BindTexture(ffi::TEXTURE_2D, tex);
            self.gl.TexImage2D(
                ffi::TEXTURE_2D,
                0,
                internal as i32,
                size.w,
                size.h,
                0,
                format,
                layout,
                std::ptr::null(),
            );
            tex
        };

        Ok(unsafe { GlesTexture::from_raw(self, Some(internal), !has_alpha, tex, size) })
    }
}

impl Offscreen<GlesRenderbuffer> for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn create_buffer(
        &mut self,
        format: Fourcc,
        size: Size<i32, BufferCoord>,
    ) -> Result<GlesRenderbuffer, GlesError> {
        if !self.capabilities.contains(&Capability::Renderbuffer) {
            return Err(GlesError::UnsupportedPixelFormat(format));
        }
        let has_alpha = has_alpha(format);
        let (internal, _, _) =
            fourcc_to_gl_formats(format).ok_or(GlesError::UnsupportedPixelFormat(format))?;

        if internal != ffi::RGBA8 && !self.capabilities.contains(&Capability::_10Bit) {
            return Err(GlesError::UnsupportedPixelLayout);
        }

        unsafe {
            self.egl.make_current()?;

            let mut rbo = 0;
            self.gl.GenRenderbuffers(1, &mut rbo);
            self.gl.BindRenderbuffer(ffi::RENDERBUFFER, rbo);
            self.gl
                .RenderbufferStorage(ffi::RENDERBUFFER, internal, size.w, size.h);
            self.gl.BindRenderbuffer(ffi::RENDERBUFFER, 0);

            Ok(GlesRenderbuffer(Rc::new(GlesRenderbufferInternal {
                rbo,
                format: internal,
                has_alpha,
                size,
                destruction_callback_sender: self.destruction_callback_sender.clone(),
            })))
        }
    }
}

impl<'buffer> BlitFrame<GlesTarget<'buffer>> for GlesFrame<'_, 'buffer> {
    fn blit_to(
        &mut self,
        to: &mut GlesTarget<'buffer>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), Self::Error> {
        let res = self.renderer.blit(self.target, to, src, dst, filter);
        self.target
            .0
            .make_current(&self.renderer.gl, &self.renderer.egl)?;
        res
    }

    fn blit_from(
        &mut self,
        from: &GlesTarget<'buffer>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), Self::Error> {
        let res = self.renderer.blit(from, self.target, src, dst, filter);
        self.target
            .0
            .make_current(&self.renderer.gl, &self.renderer.egl)?;
        res
    }
}

impl Blit for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self, src_target, dst_target))]
    #[profiling::function]
    fn blit(
        &mut self,
        src_target: &GlesTarget<'_>,
        dst_target: &mut GlesTarget<'_>,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), GlesError> {
        // glBlitFramebuffer is sadly only available for GLES 3.0 and higher
        if self.gl_version < version::GLES_3_0 {
            return Err(GlesError::GLVersionNotSupported(version::GLES_3_0));
        }

        match (&src_target.0, &dst_target.0) {
            (
                GlesTargetInternal::Surface { surface: src, .. },
                GlesTargetInternal::Surface { surface: dst, .. },
            ) => unsafe {
                self.egl.make_current_with_draw_and_read_surface(dst, src)?;
            },
            (GlesTargetInternal::Surface { surface: src, .. }, _) => unsafe {
                self.egl.make_current_with_surface(src)?;
            },
            (_, GlesTargetInternal::Surface { surface: dst, .. }) => unsafe {
                self.egl.make_current_with_surface(dst)?;
            },
            (_, _) => unsafe {
                self.egl.make_current()?;
            },
        }

        match &src_target.0 {
            GlesTargetInternal::Image { ref buf, .. } => unsafe {
                self.gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, buf.fbo)
            },
            GlesTargetInternal::Texture { ref fbo, .. } => unsafe {
                self.gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, *fbo)
            },
            GlesTargetInternal::Renderbuffer { ref fbo, .. } => unsafe {
                self.gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, *fbo)
            },
            _ => {} // Note: The only target missing is `Surface` and handled above
        }
        match &dst_target.0 {
            GlesTargetInternal::Image { ref buf, .. } => unsafe {
                self.gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, buf.fbo)
            },
            GlesTargetInternal::Texture { ref fbo, .. } => unsafe {
                self.gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, *fbo)
            },
            GlesTargetInternal::Renderbuffer { ref fbo, .. } => unsafe {
                self.gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, *fbo)
            },
            _ => {} // Note: The only target missing is `Surface` and handled above
        }

        let status = unsafe { self.gl.CheckFramebufferStatus(ffi::FRAMEBUFFER) };
        if status != ffi::FRAMEBUFFER_COMPLETE {
            let _ = self.unbind();
            return Err(GlesError::FramebufferBindingError);
        }

        let errno = unsafe {
            while self.gl.GetError() != ffi::NO_ERROR {} // clear flag before
            self.gl.BlitFramebuffer(
                src.loc.x,
                src.loc.y,
                src.loc.x + src.size.w,
                src.loc.y + src.size.h,
                dst.loc.x,
                dst.loc.y,
                dst.loc.x + dst.size.w,
                dst.loc.y + dst.size.h,
                ffi::COLOR_BUFFER_BIT,
                match filter {
                    TextureFilter::Linear => ffi::LINEAR,
                    TextureFilter::Nearest => ffi::NEAREST,
                },
            );
            self.gl.GetError()
        };

        if errno == ffi::INVALID_OPERATION {
            Err(GlesError::BlitError)
        } else {
            Ok(())
        }
    }
}

impl Drop for GlesRenderer {
    fn drop(&mut self) {
        let _guard = self.span.enter();
        unsafe {
            if self.egl.make_current().is_ok() {
                self.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
                self.gl.DeleteProgram(self.solid_program.program);
                self.gl.DeleteBuffers(self.vbos.len() as i32, self.vbos.as_ptr());

                if self.extensions.iter().any(|ext| ext == "GL_KHR_debug") {
                    self.gl.Disable(ffi::DEBUG_OUTPUT);
                    self.gl.DebugMessageCallback(None, ptr::null());
                }

                #[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
                let _ = self.egl_reader.take();
                let _ = self.egl.unbind();
            }

            if let Some(gl_debug_ptr) = self.gl_debug_span.take() {
                let _ = Box::from_raw(gl_debug_ptr);
            }
        }
    }
}

impl GlesRenderer {
    /// Get access to the underlying [`EGLContext`].
    ///
    /// *Note*: Modifying the context state, might result in rendering issues.
    /// The context state is considerd an implementation detail
    /// and no guarantee is made about what can or cannot be changed.
    /// To make sure a certain modification does not interfere with
    /// the renderer's behaviour, check the source.
    pub fn egl_context(&self) -> &EGLContext {
        &self.egl
    }

    /// Run custom code in the GL context owned by this renderer.
    ///
    /// The OpenGL state of the renderer is considered an implementation detail
    /// and no guarantee is made about what can or cannot be changed,
    /// as such you should reset everything you change back to its previous value
    /// or check the source code of the version of Smithay you are using to ensure
    /// your changes don't interfere with the renderer's behavior.
    /// Doing otherwise can lead to rendering errors while using other functions of this renderer.
    #[instrument(level = "trace", parent = &self.span, skip_all)]
    pub fn with_context<F, R>(&mut self, func: F) -> Result<R, GlesError>
    where
        F: FnOnce(&ffi::Gles2) -> R,
    {
        unsafe {
            self.egl.make_current()?;
        }
        Ok(func(&self.gl))
    }

    /// Compile a custom pixel shader for rendering with [`GlesFrame::render_pixel_shader_to`].
    ///
    /// Pixel shaders can be used for completely shader-driven drawing into a given region.
    ///
    /// They need to handle the following #define variants:
    /// - `DEBUG_FLAGS` see below
    ///
    /// They receive the following variables:
    /// - *varying* v_coords `vec2` - contains the position from the vertex shader
    /// - *uniform* size `vec2` - size of the viewport in pixels
    /// - *uniform* alpha `float` - for the alpha value passed by the renderer
    /// - *uniform* tint `float` - for the tint passed by the renderer (either 0.0 or 1.0) - only if `DEBUG_FLAGS` was defined
    ///
    /// Additional uniform values can be defined by passing `UniformName`s to the `additional_uniforms` argument
    /// and can then be set in functions utilizing `GlesPixelProgram` (like [`GlesFrame::render_pixel_shader_to`]).
    ///
    /// The shader must **not** contain a `#version` directive. It will be interpreted as version 100.
    ///
    /// ## Panics
    ///
    /// Panics if any of the names of the passed additional uniforms contains a `\0`/NUL-byte.
    pub fn compile_custom_pixel_shader(
        &mut self,
        src: impl AsRef<str>,
        additional_uniforms: &[UniformName<'_>],
    ) -> Result<GlesPixelProgram, GlesError> {
        unsafe {
            self.egl.make_current()?;
        }

        let shader = format!("#version 100\n{}", src.as_ref());
        let program = unsafe { link_program(&self.gl, shaders::VERTEX_SHADER, &shader)? };
        let debug_shader = format!("#version 100\n#define {}\n{}", shaders::DEBUG_FLAGS, src.as_ref());
        let debug_program = unsafe { link_program(&self.gl, shaders::VERTEX_SHADER, &debug_shader)? };

        let vert = c"vert";
        let vert_position = c"vert_position";
        let matrix = c"matrix";
        let tex_matrix = c"tex_matrix";
        let size = c"size";
        let alpha = c"alpha";
        let tint = c"tint";

        unsafe {
            Ok(GlesPixelProgram(Arc::new(GlesPixelProgramInner {
                normal: GlesPixelProgramInternal {
                    program,
                    uniform_matrix: self
                        .gl
                        .GetUniformLocation(program, matrix.as_ptr() as *const ffi::types::GLchar),
                    uniform_tex_matrix: self
                        .gl
                        .GetUniformLocation(program, tex_matrix.as_ptr() as *const ffi::types::GLchar),
                    uniform_alpha: self
                        .gl
                        .GetUniformLocation(program, alpha.as_ptr() as *const ffi::types::GLchar),
                    uniform_size: self
                        .gl
                        .GetUniformLocation(program, size.as_ptr() as *const ffi::types::GLchar),
                    attrib_vert: self
                        .gl
                        .GetAttribLocation(program, vert.as_ptr() as *const ffi::types::GLchar),
                    attrib_position: self
                        .gl
                        .GetAttribLocation(program, vert_position.as_ptr() as *const ffi::types::GLchar),
                    additional_uniforms: additional_uniforms
                        .iter()
                        .map(|uniform| {
                            let name = CString::new(uniform.name.as_bytes()).expect("Interior null in name");
                            let location = self
                                .gl
                                .GetUniformLocation(program, name.as_ptr() as *const ffi::types::GLchar);
                            (
                                uniform.name.clone().into_owned(),
                                UniformDesc {
                                    location,
                                    type_: uniform.type_,
                                },
                            )
                        })
                        .collect(),
                },
                debug: GlesPixelProgramInternal {
                    program: debug_program,
                    uniform_matrix: self
                        .gl
                        .GetUniformLocation(debug_program, matrix.as_ptr() as *const ffi::types::GLchar),
                    uniform_tex_matrix: self
                        .gl
                        .GetUniformLocation(debug_program, tex_matrix.as_ptr() as *const ffi::types::GLchar),
                    uniform_alpha: self
                        .gl
                        .GetUniformLocation(debug_program, alpha.as_ptr() as *const ffi::types::GLchar),
                    uniform_size: self
                        .gl
                        .GetUniformLocation(debug_program, size.as_ptr() as *const ffi::types::GLchar),
                    attrib_vert: self
                        .gl
                        .GetAttribLocation(debug_program, vert.as_ptr() as *const ffi::types::GLchar),
                    attrib_position: self.gl.GetAttribLocation(
                        debug_program,
                        vert_position.as_ptr() as *const ffi::types::GLchar,
                    ),
                    additional_uniforms: additional_uniforms
                        .iter()
                        .map(|uniform| {
                            let name = CString::new(uniform.name.as_bytes()).expect("Interior null in name");
                            let location = self.gl.GetUniformLocation(
                                debug_program,
                                name.as_ptr() as *const ffi::types::GLchar,
                            );
                            (
                                uniform.name.clone().into_owned(),
                                UniformDesc {
                                    location,
                                    type_: uniform.type_,
                                },
                            )
                        })
                        .collect(),
                },
                destruction_callback_sender: self.destruction_callback_sender.clone(),
                uniform_tint: self
                    .gl
                    .GetUniformLocation(debug_program, tint.as_ptr() as *const ffi::types::GLchar),
            })))
        }
    }

    /// Compile a custom texture shader for rendering with [`GlesFrame::render_texture`] or [`GlesFrame::render_texture_from_to`].
    ///
    /// They need to handle the following #define variants:
    /// - `EXTERNAL` uses samplerExternalOES instead of sampler2D, requires the GL_OES_EGL_image_external extension
    /// - `NO_ALPHA` needs to ignore the alpha channel of the texture and replace it with 1.0
    /// - `DEBUG_FLAGS` see below
    ///
    /// They receive the following variables:
    /// - *varying* v_coords `vec2` - contains the position from the vertex shader
    /// - *uniform* tex `sample2d` - texture sampler
    /// - *uniform* alpha `float` - for the alpha value passed by the renderer
    /// - *uniform* tint `float` - for the tint passed by the renderer (either 0.0 or 1.0) - only if `DEBUG_FLAGS` was defined
    ///
    /// Additional uniform values can be defined by passing `UniformName`s to the `additional_uniforms` argument
    /// and can then be set in functions utilizing `GlesTexProgram` (like [`GlesFrame::render_texture`] or [`GlesFrame::render_texture_from_to`]).
    ///
    /// The shader must contain a line only containing `//_DEFINES`. It will be replaced by the renderer with corresponding `#define` directives.
    ///
    /// ## Panics
    ///
    /// Panics if any of the names of the passed additional uniforms contains a `\0`/NUL-byte.
    pub fn compile_custom_texture_shader(
        &mut self,
        shader: impl AsRef<str>,
        additional_uniforms: &[UniformName<'_>],
    ) -> Result<GlesTexProgram, GlesError> {
        unsafe {
            self.egl.make_current()?;
        }

        unsafe {
            texture_program(
                &self.gl,
                shader.as_ref(),
                additional_uniforms,
                self.destruction_callback_sender.clone(),
            )
        }
    }
}

impl GlesFrame<'_, '_> {
    /// Run custom code in the GL context owned by this renderer.
    ///
    /// The OpenGL state of the renderer is considered an implementation detail
    /// and no guarantee is made about what can or cannot be changed,
    /// as such you should reset everything you change back to its previous value
    /// or check the source code of the version of Smithay you are using to ensure
    /// your changes don't interfere with the renderer's behavior.
    /// Doing otherwise can lead to rendering errors while using other functions of this renderer.
    #[instrument(level = "trace", parent = &self.span, skip_all)]
    pub fn with_context<F, R>(&mut self, func: F) -> Result<R, GlesError>
    where
        F: FnOnce(&ffi::Gles2) -> R,
    {
        Ok(func(&self.renderer.gl))
    }
}

impl RendererSuper for GlesRenderer {
    type Error = GlesError;
    type TextureId = GlesTexture;
    type Framebuffer<'buffer> = GlesTarget<'buffer>;
    type Frame<'frame, 'buffer>
        = GlesFrame<'frame, 'buffer>
    where
        'buffer: 'frame;
}

impl Renderer for GlesRenderer {
    fn context_id(&self) -> ContextId<GlesTexture> {
        self.egl
            .user_data()
            .get::<ContextId<GlesTexture>>()
            .unwrap()
            .clone()
    }

    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.min_filter = filter;
        Ok(())
    }
    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.max_filter = filter;
        Ok(())
    }

    fn set_debug_flags(&mut self, flags: DebugFlags) {
        self.debug_flags = flags;
    }

    fn debug_flags(&self) -> DebugFlags {
        self.debug_flags
    }

    #[profiling::function]
    fn render<'frame, 'buffer>(
        &'frame mut self,
        target: &'frame mut GlesTarget<'buffer>,
        mut output_size: Size<i32, Physical>,
        transform: Transform,
    ) -> Result<GlesFrame<'frame, 'buffer>, GlesError>
    where
        'buffer: 'frame,
    {
        target.0.make_current(&self.gl, &self.egl)?;

        unsafe {
            self.gl.Viewport(0, 0, output_size.w, output_size.h);

            self.gl.Scissor(0, 0, output_size.w, output_size.h);
            self.gl.Enable(ffi::SCISSOR_TEST);

            self.gl.Enable(ffi::BLEND);
            self.gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
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

        Ok(GlesFrame {
            renderer: self,
            target,
            // output transformation passed in by the user
            current_projection,
            transform,
            size: output_size,
            tex_program_override: None,
            finished: AtomicBool::new(false),

            span,
        })
    }

    #[profiling::function]
    fn wait(&mut self, sync: &super::sync::SyncPoint) -> Result<(), Self::Error> {
        unsafe {
            self.egl.make_current()?;
        }

        let display = self.egl_context().display();

        // if the sync point holds a EGLFence we can try
        // to directly insert it in our context
        if let Some(fence) = sync.get::<EGLFence>() {
            if fence.wait(display).is_ok() {
                return Ok(());
            }
        }

        // alternative we try to create a temporary fence
        // out of the native fence if available and try
        // to insert it in our context
        if let Some(native) = EGLFence::supports_importing(display)
            .then(|| sync.export())
            .flatten()
        {
            if let Ok(fence) = EGLFence::import(display, native) {
                if fence.wait(display).is_ok() {
                    return Ok(());
                }
            }
        }

        // if everything above failed we can only
        // block until the sync point has been reached
        sync.wait().map_err(|_| GlesError::SyncInterrupted)
    }

    #[profiling::function]
    fn cleanup_texture_cache(&mut self) -> Result<(), Self::Error> {
        unsafe {
            self.egl.make_current()?;
        }
        self.cleanup();
        Ok(())
    }
}

/// Vertices for instanced rendering.
static INSTANCED_VERTS: [ffi::types::GLfloat; 8] = [
    1.0, 0.0, // top right
    0.0, 0.0, // top left
    1.0, 1.0, // bottom right
    0.0, 1.0, // bottom left
];

/// Vertices for rendering individual triangles.
const MAX_RECTS_PER_DRAW: usize = 10;
const TRIANGLE_VERTS: [ffi::types::GLfloat; 12 * MAX_RECTS_PER_DRAW] = triangle_verts();
const fn triangle_verts() -> [ffi::types::GLfloat; 12 * MAX_RECTS_PER_DRAW] {
    let mut verts = [0.; 12 * MAX_RECTS_PER_DRAW];
    let mut i = 0;
    loop {
        // Top Left.
        verts[i * 12] = 0.0;
        verts[i * 12 + 1] = 0.0;

        // Bottom left.
        verts[i * 12 + 2] = 0.0;
        verts[i * 12 + 3] = 1.0;

        // Bottom right.
        verts[i * 12 + 4] = 1.0;
        verts[i * 12 + 5] = 1.0;

        // Top left.
        verts[i * 12 + 6] = 0.0;
        verts[i * 12 + 7] = 0.0;

        // Bottom right.
        verts[i * 12 + 8] = 1.0;
        verts[i * 12 + 9] = 1.0;

        // Top right.
        verts[i * 12 + 10] = 1.0;
        verts[i * 12 + 11] = 0.0;

        i += 1;
        if i == MAX_RECTS_PER_DRAW {
            break;
        }
    }
    verts
}

/// Vertices for output rendering.
static OUTPUT_VERTS: [ffi::types::GLfloat; 8] = [
    -1.0, 1.0, // top right
    -1.0, -1.0, // top left
    1.0, 1.0, // bottom right
    1.0, -1.0, // bottom left
];

impl Frame for GlesFrame<'_, '_> {
    type Error = GlesError;
    type TextureId = GlesTexture;

    fn context_id(&self) -> ContextId<GlesTexture> {
        self.renderer.context_id()
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn clear(&mut self, color: Color32F, at: &[Rectangle<i32, Physical>]) -> Result<(), GlesError> {
        if at.is_empty() {
            return Ok(());
        }

        unsafe {
            self.renderer.gl.Disable(ffi::BLEND);
        }

        let res = self.draw_solid(Rectangle::from_size(self.size), at, color);

        unsafe {
            self.renderer.gl.Enable(ffi::BLEND);
            self.renderer.gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
        }
        res
    }

    #[instrument(level = "trace", skip(self), parent = &self.span)]
    #[profiling::function]
    fn draw_solid(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: Color32F,
    ) -> Result<(), Self::Error> {
        if damage.is_empty() {
            return Ok(());
        }

        let is_opaque = color.is_opaque();

        if is_opaque {
            unsafe {
                self.renderer.gl.Disable(ffi::BLEND);
            }
        }

        let res = self.draw_solid(dst, damage, color);

        if is_opaque {
            unsafe {
                self.renderer.gl.Enable(ffi::BLEND);
                self.renderer.gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
            }
        }

        res
    }

    #[instrument(level = "trace", skip(self), parent = &self.span)]
    #[profiling::function]
    fn render_texture_from_to(
        &mut self,
        texture: &GlesTexture,
        src: Rectangle<f64, BufferCoord>,
        dest: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        transform: Transform,
        alpha: f32,
    ) -> Result<(), GlesError> {
        self.render_texture_from_to(
            texture,
            src,
            dest,
            damage,
            opaque_regions,
            transform,
            alpha,
            None,
            &[],
        )
    }

    fn transformation(&self) -> Transform {
        self.transform
    }

    #[profiling::function]
    fn wait(&mut self, sync: &SyncPoint) -> Result<(), Self::Error> {
        self.renderer.wait(sync)
    }

    #[profiling::function]
    fn finish(mut self) -> Result<SyncPoint, Self::Error> {
        self.finish_internal()
    }
}

impl GlesFrame<'_, '_> {
    #[profiling::function]
    fn finish_internal(&mut self) -> Result<SyncPoint, GlesError> {
        let _guard = self.span.enter();

        if self.finished.swap(true, Ordering::SeqCst) {
            return Ok(SyncPoint::signaled());
        }

        unsafe {
            self.renderer.gl.Disable(ffi::SCISSOR_TEST);
            self.renderer.gl.Disable(ffi::BLEND);
        }

        if let GlesTargetInternal::Texture { sync_lock, .. } = &mut self.target.0 {
            sync_lock.update_write(&self.renderer.gl);
        }

        // delayed destruction until the next frame rendering.
        self.renderer.cleanup();

        // if we support egl fences we should use it
        if self.renderer.capabilities.contains(&Capability::ExportFence) {
            if let Ok(fence) = EGLFence::create(self.renderer.egl.display()) {
                unsafe {
                    self.renderer.gl.Flush();
                }
                return Ok(SyncPoint::from(fence));
            }
        }

        // as a last option we force finish, this is unlikely to happen
        unsafe {
            self.renderer.gl.Finish();
        }
        Ok(SyncPoint::signaled())
    }

    /// Overrides the default texture shader used, if none is specified.
    ///
    /// This affects calls to [`Frame::render_texture_at`] or [`Frame::render_texture_from_to`] as well as
    /// calls to [`GlesFrame::render_texture_from_to`] or [`GlesFrame::render_texture`], if the passed in `program` is `None`.
    ///
    /// Override is active only for the lifetime of this `GlesFrame` and can be reset via [`GlesFrame::clear_tex_program_override`].
    pub fn override_default_tex_program(
        &mut self,
        program: GlesTexProgram,
        additional_uniforms: Vec<Uniform<'static>>,
    ) {
        self.tex_program_override = Some((program, additional_uniforms));
    }

    /// Resets a texture shader override previously set by [`GlesFrame::override_default_tex_program`].
    pub fn clear_tex_program_override(&mut self) {
        self.tex_program_override = None;
    }

    /// Draw a solid color to the current target at the specified destination with the specified color.
    #[instrument(level = "trace", skip(self), parent = &self.span)]
    #[profiling::function]
    pub fn draw_solid(
        &mut self,
        dest: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: Color32F,
    ) -> Result<(), GlesError> {
        if damage.is_empty() {
            return Ok(());
        }

        let mut mat = Matrix3::<f32>::identity();
        mat = self.current_projection * mat;

        // prepare the vertices
        self.renderer.vertices.clear();
        if self.renderer.capabilities.contains(&Capability::Instancing) {
            self.renderer.vertices.extend(damage.iter().flat_map(|rect| {
                let dest_size = dest.size;

                let rect_constrained_loc = rect.loc.constrain(Rectangle::from_size(dest_size));
                let rect_clamped_size = rect
                    .size
                    .clamp((0, 0), (dest_size.to_point() - rect_constrained_loc).to_size());

                let rect = Rectangle::new(rect_constrained_loc, rect_clamped_size);
                [
                    (dest.loc.x + rect.loc.x) as f32,
                    (dest.loc.y + rect.loc.y) as f32,
                    rect.size.w as f32,
                    rect.size.h as f32,
                ]
            }))
        } else {
            self.renderer.vertices.extend(damage.iter().flat_map(|rect| {
                let dest_size = dest.size;

                let rect_constrained_loc = rect.loc.constrain(Rectangle::from_size(dest_size));
                let rect_clamped_size = rect
                    .size
                    .clamp((0, 0), (dest_size.to_point() - rect_constrained_loc).to_size());

                let rect = Rectangle::new(rect_constrained_loc, rect_clamped_size);
                // Add the 4 f32s per damage rectangle for each of the 6 vertices.
                (0..6).flat_map(move |_| {
                    [
                        (dest.loc.x + rect.loc.x) as f32,
                        (dest.loc.y + rect.loc.y) as f32,
                        rect.size.w as f32,
                        rect.size.h as f32,
                    ]
                })
            }));
        }

        let gl = &self.renderer.gl;
        unsafe {
            gl.UseProgram(self.renderer.solid_program.program);
            gl.Uniform4f(
                self.renderer.solid_program.uniform_color,
                color.r(),
                color.g(),
                color.b(),
                color.a(),
            );
            gl.UniformMatrix3fv(
                self.renderer.solid_program.uniform_matrix,
                1,
                ffi::FALSE,
                mat.as_ptr(),
            );

            gl.EnableVertexAttribArray(self.renderer.solid_program.attrib_vert as u32);
            gl.BindBuffer(ffi::ARRAY_BUFFER, self.renderer.vbos[0]);
            gl.VertexAttribPointer(
                self.renderer.solid_program.attrib_vert as u32,
                2,
                ffi::FLOAT,
                ffi::FALSE,
                0,
                std::ptr::null(),
            );

            gl.EnableVertexAttribArray(self.renderer.solid_program.attrib_position as u32);
            gl.BindBuffer(ffi::ARRAY_BUFFER, 0);

            gl.VertexAttribPointer(
                self.renderer.solid_program.attrib_position as u32,
                4,
                ffi::FLOAT,
                ffi::FALSE,
                0,
                self.renderer.vertices.as_ptr() as *const _,
            );

            let damage_len = damage.len() as i32;
            if self.renderer.capabilities.contains(&Capability::Instancing) {
                gl.VertexAttribDivisor(self.renderer.solid_program.attrib_vert as u32, 0);

                gl.VertexAttribDivisor(self.renderer.solid_program.attrib_position as u32, 1);

                gl.DrawArraysInstanced(ffi::TRIANGLE_STRIP, 0, 4, damage_len);
            } else {
                let count = damage_len * 6;
                gl.DrawArrays(ffi::TRIANGLES, 0, count);
            }

            gl.DisableVertexAttribArray(self.renderer.solid_program.attrib_vert as u32);
            gl.DisableVertexAttribArray(self.renderer.solid_program.attrib_position as u32);
        }

        Ok(())
    }

    /// Render part of a texture as given by src to the current target into the rectangle described by dst
    /// as a flat 2d-plane after applying the inverse of the given transformation.
    /// (Meaning `src_transform` should match the orientation of surface being rendered).
    ///
    /// Optionally allows a custom texture program and matching additional uniforms to be passed in.
    #[instrument(level = "trace", skip(self), parent = &self.span)]
    #[profiling::function]
    #[allow(clippy::too_many_arguments)]
    pub fn render_texture_from_to(
        &mut self,
        texture: &GlesTexture,
        src: Rectangle<f64, BufferCoord>,
        dest: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        transform: Transform,
        alpha: f32,
        program: Option<&GlesTexProgram>,
        additional_uniforms: &[Uniform<'_>],
    ) -> Result<(), GlesError> {
        let mut mat = Matrix3::<f32>::identity();

        // dest position and scale
        mat = mat * Matrix3::from_translation(Vector2::new(dest.loc.x as f32, dest.loc.y as f32));

        // src scale, position, tranform and y_inverted
        let tex_size = texture.size();
        let src_size = src.size;

        if src_size.is_empty() || tex_size.is_empty() {
            return Ok(());
        }

        let mut tex_mat = build_texture_mat(src, dest, tex_size, transform);
        if texture.0.y_inverted {
            tex_mat = Matrix3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0) * tex_mat;
        }

        let render_texture = |renderer: &mut Self, damage: &[Rectangle<i32, Physical>]| {
            let instances = damage.iter().flat_map(|rect| {
                let dest_size = dest.size;

                let rect_constrained_loc = rect.loc.constrain(Rectangle::from_size(dest_size));
                let rect_clamped_size = rect
                    .size
                    .clamp((0, 0), (dest_size.to_point() - rect_constrained_loc).to_size());

                let rect = Rectangle::new(rect_constrained_loc, rect_clamped_size);
                [
                    rect.loc.x as f32,
                    rect.loc.y as f32,
                    rect.size.w as f32,
                    rect.size.h as f32,
                ]
            });

            renderer.render_texture(
                texture,
                tex_mat,
                mat,
                Some(instances),
                alpha,
                program,
                additional_uniforms,
            )
        };

        // We split the damage in opaque and non opaque regions, for opaque regions we can
        // disable blending. Most likely we did not clear regions marked as opaque, which can
        // result in read-back when not disabling blending. This can be problematic on tile based
        // renderers.
        let mut non_opaque_damage = std::mem::take(&mut self.renderer.non_opaque_damage);
        let mut opaque_damage = std::mem::take(&mut self.renderer.opaque_damage);
        non_opaque_damage.clear();
        opaque_damage.clear();

        // If drawing is implicit opaque and we have no custom program we
        // can skip some logic and save a few operations. In case we have
        // some user-provided alpha we can not disable blending, but should
        // also have cleared the region previously anyway. In case we have
        // no opaque regions we can also short cut the logic a bit.
        let is_implicit_opaque = !texture.0.has_alpha && alpha == 1f32;
        if is_implicit_opaque && program.is_none() && self.tex_program_override.is_none() {
            opaque_damage.extend_from_slice(damage);
        } else if alpha != 1f32 || opaque_regions.is_empty() {
            non_opaque_damage.extend_from_slice(damage);
        } else {
            non_opaque_damage.extend_from_slice(damage);
            opaque_damage.extend_from_slice(damage);

            non_opaque_damage =
                Rectangle::subtract_rects_many_in_place(non_opaque_damage, opaque_regions.iter().copied());
            opaque_damage =
                Rectangle::subtract_rects_many_in_place(opaque_damage, non_opaque_damage.iter().copied());
        }

        tracing::trace!(non_opaque_damage = ?non_opaque_damage, opaque_damage = ?opaque_damage, "drawing texture");

        let non_opaque_render_res = if !non_opaque_damage.is_empty() {
            render_texture(self, &non_opaque_damage)
        } else {
            Ok(())
        };

        let opaque_render_res = if !opaque_damage.is_empty() {
            unsafe {
                self.renderer.gl.Disable(ffi::BLEND);
            }

            let res = render_texture(self, &opaque_damage);

            unsafe {
                self.renderer.gl.Enable(ffi::BLEND);
                self.renderer.gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
            }

            res
        } else {
            Ok(())
        };

        // Return the damage(s) to be able to re-use the allocation(s)
        std::mem::swap(&mut self.renderer.non_opaque_damage, &mut non_opaque_damage);
        std::mem::swap(&mut self.renderer.opaque_damage, &mut opaque_damage);

        non_opaque_render_res?;
        opaque_render_res?;

        Ok(())
    }

    /// Render a texture to the current target using given projection matrix and alpha.
    ///
    /// The instances are used to define the regions which should get drawn.
    /// Each instance has to define 4 [`GLfloat`](ffi::types::GLfloat) which define the
    /// relative offset and scale for the vertex position and range from `0.0` to `1.0`.
    /// The first 2 [`GLfloat`](ffi::types::GLfloat) define the relative x and y offset.
    /// The remaining 2 [`GLfloat`](ffi::types::GLfloat) define the x and y scale.
    /// This can be used to only update parts of the texture on screen.
    ///
    /// The given texture matrix is used to transform the instances into texture coordinates.
    /// In case the texture is rotated, flipped or y-inverted the matrix has to be set up accordingly.
    /// Additionally the matrix can be used to crop the texture.
    ///
    /// Optionally allows a custom texture program and matching additional uniforms to be passed in.
    #[instrument(level = "trace", skip(self, instances), parent = &self.span)]
    #[profiling::function]
    #[allow(clippy::too_many_arguments)]
    pub fn render_texture(
        &mut self,
        tex: &GlesTexture,
        tex_matrix: Matrix3<f32>,
        mut matrix: Matrix3<f32>,
        instances: Option<impl IntoIterator<Item = ffi::types::GLfloat>>,
        alpha: f32,
        program: Option<&GlesTexProgram>,
        additional_uniforms: &[Uniform<'_>],
    ) -> Result<(), GlesError> {
        // prepare the vertices
        self.renderer.vertices.clear();
        let damage_len = if let Some(instances) = instances {
            if self.renderer.capabilities.contains(&Capability::Instancing) {
                self.renderer.vertices.extend(instances);
                self.renderer.vertices.len() / 4
            } else {
                let mut damage = 0;
                let mut instances = instances.into_iter();
                while let Some(first) = instances.next() {
                    damage += 1;
                    let vertices = [
                        first,
                        instances.next().unwrap(),
                        instances.next().unwrap(),
                        instances.next().unwrap(),
                    ];
                    // Add the 4 f32s per damage rectangle for each of the 6 vertices.
                    for _ in 0..6 {
                        self.renderer.vertices.extend_from_slice(&vertices);
                    }
                }
                damage
            }
        } else if self.renderer.capabilities.contains(&Capability::Instancing) {
            self.renderer.vertices.extend_from_slice(&[0.0, 0.0, 1.0, 1.0]);
            1
        } else {
            // Add the 4 f32s per damage rectangle for each of the 6 vertices.
            for _ in 0..6 {
                self.renderer.vertices.extend_from_slice(&[0.0, 0.0, 1.0, 1.0]);
            }
            1
        };

        if self.renderer.vertices.is_empty() {
            return Ok(());
        }

        //apply output transformation
        matrix = self.current_projection * matrix;

        let target = if tex.0.is_external {
            ffi::TEXTURE_EXTERNAL_OES
        } else {
            ffi::TEXTURE_2D
        };
        let (tex_program, additional_uniforms) = program
            .map(|p| (p, additional_uniforms))
            .or_else(|| self.tex_program_override.as_ref().map(|(p, a)| (p, &**a)))
            .unwrap_or((&self.renderer.tex_program, &[]));
        let program_variant = tex_program.variant_for_format(
            if !tex.0.is_external { tex.0.format } else { None },
            tex.0.has_alpha,
        );
        let program = if self.renderer.debug_flags.is_empty() {
            &program_variant.normal
        } else {
            &program_variant.debug
        };

        // render
        let gl = &self.renderer.gl;
        let sync_lock = tex.0.sync.read().unwrap();
        unsafe {
            sync_lock.wait_for_upload(gl);
            gl.ActiveTexture(ffi::TEXTURE0);
            gl.BindTexture(target, tex.0.texture);
            gl.TexParameteri(
                target,
                ffi::TEXTURE_MIN_FILTER,
                match self.renderer.min_filter {
                    TextureFilter::Nearest => ffi::NEAREST as i32,
                    TextureFilter::Linear => ffi::LINEAR as i32,
                },
            );
            gl.TexParameteri(
                target,
                ffi::TEXTURE_MAG_FILTER,
                match self.renderer.max_filter {
                    TextureFilter::Nearest => ffi::NEAREST as i32,
                    TextureFilter::Linear => ffi::LINEAR as i32,
                },
            );
            gl.UseProgram(program.program);

            gl.Uniform1i(program.uniform_tex, 0);
            gl.UniformMatrix3fv(program.uniform_matrix, 1, ffi::FALSE, matrix.as_ptr());
            gl.UniformMatrix3fv(program.uniform_tex_matrix, 1, ffi::FALSE, tex_matrix.as_ptr());
            gl.Uniform1f(program.uniform_alpha, alpha);

            if !self.renderer.debug_flags.is_empty() {
                let tint = if self.renderer.debug_flags.contains(DebugFlags::TINT) {
                    1.0f32
                } else {
                    0.0f32
                };
                gl.Uniform1f(program_variant.uniform_tint, tint);
            }

            for uniform in additional_uniforms {
                let desc = program
                    .additional_uniforms
                    .get(&*uniform.name)
                    .ok_or_else(|| GlesError::UnknownUniform(uniform.name.clone().into_owned()))?;
                uniform.value.set(gl, desc)?;
            }

            gl.EnableVertexAttribArray(program.attrib_vert as u32);
            gl.BindBuffer(ffi::ARRAY_BUFFER, self.renderer.vbos[0]);
            gl.VertexAttribPointer(
                program.attrib_vert as u32,
                2,
                ffi::FLOAT,
                ffi::FALSE,
                0,
                std::ptr::null(),
            );

            // vert_position
            gl.EnableVertexAttribArray(program.attrib_vert_position as u32);
            gl.BindBuffer(ffi::ARRAY_BUFFER, 0);

            gl.VertexAttribPointer(
                program.attrib_vert_position as u32,
                4,
                ffi::FLOAT,
                ffi::FALSE,
                0,
                self.renderer.vertices.as_ptr() as *const _,
            );

            if self.renderer.capabilities.contains(&Capability::Instancing) {
                gl.VertexAttribDivisor(program.attrib_vert as u32, 0);
                gl.VertexAttribDivisor(program.attrib_vert_position as u32, 1);

                gl.DrawArraysInstanced(ffi::TRIANGLE_STRIP, 0, 4, damage_len as i32);
            } else {
                let count = damage_len * 6;
                gl.DrawArrays(ffi::TRIANGLES, 0, count as i32);
            }

            gl.BindTexture(target, 0);
            gl.DisableVertexAttribArray(program.attrib_vert as u32);
            gl.DisableVertexAttribArray(program.attrib_vert_position as u32);

            if self.renderer.capabilities.contains(&Capability::Fencing) {
                sync_lock.update_read(gl);
            } else if self.renderer.egl.is_shared() {
                gl.Finish();
            };
        }

        Ok(())
    }

    /// Render a pixel shader into the current target at a given `dest`-region.
    #[profiling::function]
    #[allow(clippy::too_many_arguments)]
    pub fn render_pixel_shader_to(
        &mut self,
        pixel_shader: &GlesPixelProgram,
        src: Rectangle<f64, BufferCoord>,
        dest: Rectangle<i32, Physical>,
        size: Size<i32, BufferCoord>,
        damage: Option<&[Rectangle<i32, Physical>]>,
        alpha: f32,
        additional_uniforms: &[Uniform<'_>],
    ) -> Result<(), GlesError> {
        let fallback_damage = &[Rectangle::from_size(dest.size)];
        let damage = damage.unwrap_or(fallback_damage);

        // prepare the vertices
        self.renderer.vertices.clear();
        if self.renderer.capabilities.contains(&Capability::Instancing) {
            self.renderer.vertices.extend(damage.iter().flat_map(|rect| {
                let dest_size = dest.size;

                let rect_constrained_loc = rect.loc.constrain(Rectangle::from_size(dest_size));
                let rect_clamped_size = rect
                    .size
                    .clamp((0, 0), (dest_size.to_point() - rect_constrained_loc).to_size());

                let rect = Rectangle::new(rect_constrained_loc, rect_clamped_size);
                [
                    rect.loc.x as f32,
                    rect.loc.y as f32,
                    rect.size.w as f32,
                    rect.size.h as f32,
                ]
            }));
        } else {
            self.renderer.vertices.extend(damage.iter().flat_map(|rect| {
                let dest_size = dest.size;

                let rect_constrained_loc = rect.loc.constrain(Rectangle::from_size(dest_size));
                let rect_clamped_size = rect
                    .size
                    .clamp((0, 0), (dest_size.to_point() - rect_constrained_loc).to_size());

                let rect = Rectangle::new(rect_constrained_loc, rect_clamped_size);
                // Add the 4 f32s per damage rectangle for each of the 6 vertices.
                (0..6).flat_map(move |_| {
                    [
                        rect.loc.x as f32,
                        rect.loc.y as f32,
                        rect.size.w as f32,
                        rect.size.h as f32,
                    ]
                })
            }));
        }

        if self.renderer.vertices.is_empty() {
            return Ok(());
        }

        let mut matrix = Matrix3::<f32>::identity();
        let tex_matrix = build_texture_mat(src, dest, size, Transform::Normal);

        // dest position and scale
        matrix = matrix * Matrix3::from_translation(Vector2::new(dest.loc.x as f32, dest.loc.y as f32));

        //apply output transformation
        matrix = self.current_projection * matrix;

        let program = if self.renderer.debug_flags.is_empty() {
            &pixel_shader.0.normal
        } else {
            &pixel_shader.0.debug
        };

        // render
        let gl = &self.renderer.gl;
        unsafe {
            gl.UseProgram(program.program);

            gl.UniformMatrix3fv(program.uniform_matrix, 1, ffi::FALSE, matrix.as_ptr());
            gl.UniformMatrix3fv(program.uniform_tex_matrix, 1, ffi::FALSE, tex_matrix.as_ptr());
            gl.Uniform2f(program.uniform_size, size.w as f32, size.h as f32);
            gl.Uniform1f(program.uniform_alpha, alpha);
            let tint = if self.renderer.debug_flags.contains(DebugFlags::TINT) {
                1.0f32
            } else {
                0.0f32
            };

            if !self.renderer.debug_flags.is_empty() {
                gl.Uniform1f(pixel_shader.0.uniform_tint, tint);
            }

            for uniform in additional_uniforms {
                let desc = program
                    .additional_uniforms
                    .get(&*uniform.name)
                    .ok_or_else(|| GlesError::UnknownUniform(uniform.name.clone().into_owned()))?;
                uniform.value.set(gl, desc)?;
            }

            gl.EnableVertexAttribArray(program.attrib_vert as u32);
            gl.BindBuffer(ffi::ARRAY_BUFFER, self.renderer.vbos[0]);
            gl.VertexAttribPointer(
                program.attrib_vert as u32,
                2,
                ffi::FLOAT,
                ffi::FALSE,
                0,
                std::ptr::null(),
            );

            // vert_position
            gl.EnableVertexAttribArray(program.attrib_position as u32);
            gl.BindBuffer(ffi::ARRAY_BUFFER, 0);

            gl.VertexAttribPointer(
                program.attrib_position as u32,
                4,
                ffi::FLOAT,
                ffi::FALSE,
                0,
                self.renderer.vertices.as_ptr() as *const _,
            );

            let damage_len = damage.len() as i32;
            if self.renderer.capabilities.contains(&Capability::Instancing) {
                gl.VertexAttribDivisor(program.attrib_vert as u32, 0);
                gl.VertexAttribDivisor(program.attrib_position as u32, 1);

                gl.DrawArraysInstanced(ffi::TRIANGLE_STRIP, 0, 4, damage_len);
            } else {
                let count = damage_len * 6;
                gl.DrawArrays(ffi::TRIANGLES, 0, count);
            }

            gl.DisableVertexAttribArray(program.attrib_vert as u32);
            gl.DisableVertexAttribArray(program.attrib_position as u32);
        }

        Ok(())
    }

    /// Projection matrix for this frame
    pub fn projection(&self) -> &[f32; 9] {
        self.current_projection.as_ref()
    }

    /// Get access to the underlying [`EGLContext`].
    ///
    /// *Note*: Modifying the context state, might result in rendering issues.
    /// The context state is considerd an implementation detail
    /// and no guarantee is made about what can or cannot be changed.
    /// To make sure a certain modification does not interfere with
    /// the renderer's behaviour, check the source.
    pub fn egl_context(&self) -> &EGLContext {
        self.renderer.egl_context()
    }

    /// Returns the supported [`Capabilities`](Capability) of the underlying renderer.
    pub fn capabilities(&self) -> &[Capability] {
        self.renderer.capabilities()
    }

    /// Returns the current enabled [`DebugFlags`] of the underlying renderer.
    pub fn debug_flags(&self) -> DebugFlags {
        self.renderer.debug_flags()
    }
}

impl Drop for GlesFrame<'_, '_> {
    fn drop(&mut self) {
        match self.finish_internal() {
            Ok(sync) => {
                let _ = sync.wait(); // nothing we can do
            }
            Err(err) => {
                warn!("Ignored error finishing GlesFrame on drop: {}", err);
            }
        }
    }
}

fn build_texture_mat(
    src: Rectangle<f64, BufferCoord>,
    dest: Rectangle<i32, Physical>,
    texture: Size<i32, BufferCoord>,
    transform: Transform,
) -> Matrix3<f32> {
    let dst_src_size = transform.transform_size(src.size);
    let scale = dst_src_size.to_f64() / dest.size.to_f64();

    let mut tex_mat = Matrix3::<f32>::identity();

    // first bring the damage into src scale
    tex_mat = Matrix3::from_nonuniform_scale(scale.x as f32, scale.y as f32) * tex_mat;

    // then compensate for the texture transform
    let transform_mat = transform.matrix();
    let translation = match transform {
        Transform::Normal => Matrix3::identity(),
        Transform::_90 => Matrix3::from_translation(Vector2::new(0f32, dst_src_size.w as f32)),
        Transform::_180 => {
            Matrix3::from_translation(Vector2::new(dst_src_size.w as f32, dst_src_size.h as f32))
        }
        Transform::_270 => Matrix3::from_translation(Vector2::new(dst_src_size.h as f32, 0f32)),
        Transform::Flipped => Matrix3::from_translation(Vector2::new(dst_src_size.w as f32, 0f32)),
        Transform::Flipped90 => Matrix3::identity(),
        Transform::Flipped180 => Matrix3::from_translation(Vector2::new(0f32, dst_src_size.h as f32)),
        Transform::Flipped270 => {
            Matrix3::from_translation(Vector2::new(dst_src_size.h as f32, dst_src_size.w as f32))
        }
    };
    tex_mat = transform_mat * tex_mat;
    tex_mat = translation * tex_mat;

    // now we can add the src crop loc, the size already done implicit by the src size
    tex_mat = Matrix3::from_translation(Vector2::new(src.loc.x as f32, src.loc.y as f32)) * tex_mat;

    // at last we have to normalize the values for UV space
    tex_mat = Matrix3::from_nonuniform_scale(
        (1.0f64 / texture.w as f64) as f32,
        (1.0f64 / texture.h as f64) as f32,
    ) * tex_mat;

    tex_mat
}

#[cfg(test)]
mod tests {
    use super::build_texture_mat;
    use crate::utils::{Buffer, Physical, Rectangle, Size, Transform};
    use cgmath::Vector3;

    #[test]
    fn texture_normal_double_size() {
        let src: Rectangle<f64, Buffer> = Rectangle::from_size((1000f64, 500f64).into());
        let dest: Rectangle<i32, Physical> = Rectangle::new((442, 144).into(), (500, 250).into());
        let texture_size: Size<i32, Buffer> = Size::from((1000, 500));
        let transform = Transform::Normal;

        let tex_mat = build_texture_mat(src, dest, texture_size, transform);

        let top_left = Vector3::new(0f32, 0f32, 1f32);
        let top_right = Vector3::new(dest.size.w as f32, 0f32, 1f32);
        let bottom_right = Vector3::new(dest.size.w as f32, dest.size.h as f32, 1f32);
        let bottom_left = Vector3::new(0f32, dest.size.h as f32, 1f32);

        assert_eq!(tex_mat * top_left, Vector3::new(0f32, 0f32, 1f32));
        assert_eq!(tex_mat * top_right, Vector3::new(1f32, 0f32, 1f32));
        assert_eq!(tex_mat * bottom_right, Vector3::new(1f32, 1f32, 1f32));
        assert_eq!(tex_mat * bottom_left, Vector3::new(0f32, 1f32, 1f32));
    }

    #[test]
    fn texture_scaler_crop() {
        let src: Rectangle<f64, Buffer> = Rectangle::new((42.5f64, 50.5f64).into(), (110f64, 154f64).into());
        let dest: Rectangle<i32, Physical> = Rectangle::new((813, 214).into(), (55, 77).into());
        let texture_size: Size<i32, Buffer> = Size::from((842, 674));
        let transform = Transform::Normal;

        let tex_mat = build_texture_mat(src, dest, texture_size, transform);

        let top_left = Vector3::new(0f32, 0f32, 1f32);
        let top_right = Vector3::new(dest.size.w as f32, 0f32, 1f32);
        let bottom_right = Vector3::new(dest.size.w as f32, dest.size.h as f32, 1f32);
        let bottom_left = Vector3::new(0f32, dest.size.h as f32, 1f32);

        assert_eq!(
            tex_mat * top_left,
            Vector3::new(0.05047506f32, 0.07492582f32, 1f32)
        );
        assert_eq!(
            tex_mat * top_right,
            Vector3::new(0.1811164f32, 0.07492582f32, 1f32)
        );
        assert_eq!(
            tex_mat * bottom_right,
            Vector3::new(0.1811164f32, 0.30341247f32, 1f32)
        );
        assert_eq!(
            tex_mat * bottom_left,
            Vector3::new(0.05047506f32, 0.30341247f32, 1f32)
        );
    }

    #[test]
    fn texture_normal() {
        let src: Rectangle<f64, Buffer> = Rectangle::from_size((500f64, 250f64).into());
        let dest: Rectangle<i32, Physical> = Rectangle::new((442, 144).into(), (500, 250).into());
        let texture_size: Size<i32, Buffer> = Size::from((500, 250));
        let transform = Transform::Normal;

        let tex_mat = build_texture_mat(src, dest, texture_size, transform);

        let top_left = Vector3::new(0f32, 0f32, 1f32);
        let top_right = Vector3::new(dest.size.w as f32, 0f32, 1f32);
        let bottom_right = Vector3::new(dest.size.w as f32, dest.size.h as f32, 1f32);
        let bottom_left = Vector3::new(0f32, dest.size.h as f32, 1f32);

        assert_eq!(tex_mat * top_left, Vector3::new(0f32, 0f32, 1f32));
        assert_eq!(tex_mat * top_right, Vector3::new(1f32, 0f32, 1f32));
        assert_eq!(tex_mat * bottom_right, Vector3::new(1f32, 1f32, 1f32));
        assert_eq!(tex_mat * bottom_left, Vector3::new(0f32, 1f32, 1f32));
    }

    #[test]
    fn texture_flipped() {
        let src: Rectangle<f64, Buffer> = Rectangle::from_size((500f64, 250f64).into());
        let dest: Rectangle<i32, Physical> = Rectangle::new((442, 144).into(), (500, 250).into());
        let texture_size: Size<i32, Buffer> = Size::from((500, 250));
        let transform = Transform::Flipped;

        let tex_mat = build_texture_mat(src, dest, texture_size, transform);

        let top_left = Vector3::new(0f32, 0f32, 1f32);
        let top_right = Vector3::new(dest.size.w as f32, 0f32, 1f32);
        let bottom_right = Vector3::new(dest.size.w as f32, dest.size.h as f32, 1f32);
        let bottom_left = Vector3::new(0f32, dest.size.h as f32, 1f32);

        assert_eq!(tex_mat * top_left, Vector3::new(1f32, 0f32, 1f32));
        assert_eq!(tex_mat * top_right, Vector3::new(0f32, 0f32, 1f32));
        assert_eq!(tex_mat * bottom_right, Vector3::new(0f32, 1f32, 1f32));
        assert_eq!(tex_mat * bottom_left, Vector3::new(1f32, 1f32, 1f32));
    }

    #[test]
    fn texture_90() {
        let src: Rectangle<f64, Buffer> = Rectangle::from_size((250f64, 500f64).into());
        let dest: Rectangle<i32, Physical> = Rectangle::new((442, 144).into(), (500, 250).into());
        let texture_size: Size<i32, Buffer> = Size::from((250, 500));
        let transform = Transform::_90;

        let tex_mat = build_texture_mat(src, dest, texture_size, transform);

        let top_left = Vector3::new(0f32, 0f32, 1f32);
        let top_right = Vector3::new(dest.size.w as f32, 0f32, 1f32);
        let bottom_right = Vector3::new(dest.size.w as f32, dest.size.h as f32, 1f32);
        let bottom_left = Vector3::new(0f32, dest.size.h as f32, 1f32);

        assert_eq!(tex_mat * top_left, Vector3::new(0f32, 1f32, 1f32));
        assert_eq!(tex_mat * top_right, Vector3::new(0f32, 0f32, 1f32));
        assert_eq!(tex_mat * bottom_right, Vector3::new(1f32, 0f32, 1f32));
        assert_eq!(tex_mat * bottom_left, Vector3::new(1f32, 1f32, 1f32));
    }

    #[test]
    fn texture_180() {
        let src: Rectangle<f64, Buffer> = Rectangle::from_size((500f64, 250f64).into());
        let dest: Rectangle<i32, Physical> = Rectangle::new((442, 144).into(), (500, 250).into());
        let texture_size: Size<i32, Buffer> = Size::from((500, 250));
        let transform = Transform::_180;

        let tex_mat = build_texture_mat(src, dest, texture_size, transform);

        let top_left = Vector3::new(0f32, 0f32, 1f32);
        let top_right = Vector3::new(dest.size.w as f32, 0f32, 1f32);
        let bottom_right = Vector3::new(dest.size.w as f32, dest.size.h as f32, 1f32);
        let bottom_left = Vector3::new(0f32, dest.size.h as f32, 1f32);

        assert_eq!(tex_mat * top_left, Vector3::new(1f32, 1f32, 1f32));
        assert_eq!(tex_mat * top_right, Vector3::new(0f32, 1f32, 1f32));
        assert_eq!(tex_mat * bottom_right, Vector3::new(0f32, 0f32, 1f32));
        assert_eq!(tex_mat * bottom_left, Vector3::new(1f32, 0f32, 1f32));
    }

    #[test]
    fn texture_270() {
        let src: Rectangle<f64, Buffer> = Rectangle::from_size((250f64, 500f64).into());
        let dest: Rectangle<i32, Physical> = Rectangle::new((442, 144).into(), (500, 250).into());
        let texture_size: Size<i32, Buffer> = Size::from((250, 500));
        let transform = Transform::_270;

        let tex_mat = build_texture_mat(src, dest, texture_size, transform);

        let top_left = Vector3::new(0f32, 0f32, 1f32);
        let top_right = Vector3::new(dest.size.w as f32, 0f32, 1f32);
        let bottom_right = Vector3::new(dest.size.w as f32, dest.size.h as f32, 1f32);
        let bottom_left = Vector3::new(0f32, dest.size.h as f32, 1f32);

        assert_eq!(tex_mat * top_left, Vector3::new(1f32, 0f32, 1f32));
        assert_eq!(tex_mat * top_right, Vector3::new(1f32, 1f32, 1f32));
        assert_eq!(tex_mat * bottom_right, Vector3::new(0f32, 1f32, 1f32));
        assert_eq!(tex_mat * bottom_left, Vector3::new(0f32, 0f32, 1f32));
    }

    #[test]
    fn texture_flipped_90() {
        let src: Rectangle<f64, Buffer> = Rectangle::from_size((250f64, 500f64).into());
        let dest: Rectangle<i32, Physical> = Rectangle::new((442, 144).into(), (500, 250).into());
        let texture_size: Size<i32, Buffer> = Size::from((250, 500));
        let transform = Transform::Flipped90;

        let tex_mat = build_texture_mat(src, dest, texture_size, transform);

        let top_left = Vector3::new(0f32, 0f32, 1f32);
        let top_right = Vector3::new(dest.size.w as f32, 0f32, 1f32);
        let bottom_right = Vector3::new(dest.size.w as f32, dest.size.h as f32, 1f32);
        let bottom_left = Vector3::new(0f32, dest.size.h as f32, 1f32);

        assert_eq!(tex_mat * top_left, Vector3::new(0f32, 0f32, 1f32));
        assert_eq!(tex_mat * top_right, Vector3::new(0f32, 1f32, 1f32));
        assert_eq!(tex_mat * bottom_right, Vector3::new(1f32, 1f32, 1f32));
        assert_eq!(tex_mat * bottom_left, Vector3::new(1f32, 0f32, 1f32));
    }

    #[test]
    fn texture_flipped_180() {
        let src: Rectangle<f64, Buffer> = Rectangle::from_size((500f64, 250f64).into());
        let dest: Rectangle<i32, Physical> = Rectangle::new((442, 144).into(), (500, 250).into());
        let texture_size: Size<i32, Buffer> = Size::from((500, 250));
        let transform = Transform::Flipped180;

        let tex_mat = build_texture_mat(src, dest, texture_size, transform);

        let top_left = Vector3::new(0f32, 0f32, 1f32);
        let top_right = Vector3::new(dest.size.w as f32, 0f32, 1f32);
        let bottom_right = Vector3::new(dest.size.w as f32, dest.size.h as f32, 1f32);
        let bottom_left = Vector3::new(0f32, dest.size.h as f32, 1f32);

        assert_eq!(tex_mat * top_left, Vector3::new(0f32, 1f32, 1f32));
        assert_eq!(tex_mat * top_right, Vector3::new(1f32, 1f32, 1f32));
        assert_eq!(tex_mat * bottom_right, Vector3::new(1f32, 0f32, 1f32));
        assert_eq!(tex_mat * bottom_left, Vector3::new(0f32, 0f32, 1f32));
    }

    #[test]
    fn texture_flipped_270() {
        let src: Rectangle<f64, Buffer> = Rectangle::from_size((250f64, 500f64).into());
        let dest: Rectangle<i32, Physical> = Rectangle::new((442, 144).into(), (500, 250).into());
        let texture_size: Size<i32, Buffer> = Size::from((250, 500));
        let transform = Transform::Flipped270;

        let tex_mat = build_texture_mat(src, dest, texture_size, transform);

        let top_left = Vector3::new(0f32, 0f32, 1f32);
        let top_right = Vector3::new(dest.size.w as f32, 0f32, 1f32);
        let bottom_right = Vector3::new(dest.size.w as f32, dest.size.h as f32, 1f32);
        let bottom_left = Vector3::new(0f32, dest.size.h as f32, 1f32);

        assert_eq!(tex_mat * top_left, Vector3::new(1f32, 1f32, 1f32));
        assert_eq!(tex_mat * top_right, Vector3::new(1f32, 0f32, 1f32));
        assert_eq!(tex_mat * bottom_right, Vector3::new(0f32, 0f32, 1f32));
        assert_eq!(tex_mat * bottom_left, Vector3::new(0f32, 1f32, 1f32));
    }
}
