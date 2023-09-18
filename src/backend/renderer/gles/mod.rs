//! Implementation of the rendering traits using OpenGL ES 2

use cgmath::{prelude::*, Matrix3, Vector2};
use core::slice;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    convert::TryFrom,
    ffi::{CStr, CString},
    fmt, mem,
    os::raw::c_char,
    ptr,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicPtr, Ordering},
        mpsc::{channel, Receiver, Sender},
    },
};
use tracing::{debug, error, info, info_span, instrument, span, span::EnteredSpan, trace, warn, Level};

#[cfg(feature = "wayland_frontend")]
use std::cell::RefCell;

pub mod element;
mod error;
pub mod format;
mod profiler;
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
    sync::SyncPoint, Bind, Blit, DebugFlags, ExportMem, Frame, ImportDma, ImportMem, Offscreen, Renderer,
    Texture, TextureFilter, TextureMapping, Unbind,
};
use crate::backend::egl::{
    ffi::egl::{self as ffi_egl, types::EGLImage},
    EGLContext, EGLSurface, MakeCurrentError,
};
use crate::backend::{
    allocator::{
        dmabuf::{Dmabuf, WeakDmabuf},
        format::{get_bpp, get_opaque, get_transparent, has_alpha},
        Format, Fourcc,
    },
    egl::fence::EGLFence,
};
use crate::utils::{Buffer as BufferCoord, Physical, Rectangle, Size, Transform};

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

crate::utils::ids::id_gen!(next_renderer_id, RENDERER_ID, RENDERER_IDS);
struct RendererId(usize);
impl Drop for RendererId {
    fn drop(&mut self) {
        RENDERER_IDS.lock().unwrap().remove(&self.0);
    }
}

enum CleanupResource {
    Texture(ffi::types::GLuint),
    FramebufferObject(ffi::types::GLuint),
    RenderbufferObject(ffi::types::GLuint),
    EGLImage(EGLImage),
    Mapping(ffi::types::GLuint, *const nix::libc::c_void),
    Program(ffi::types::GLuint),
}

#[derive(Debug)]
struct ShadowBuffer {
    texture: ffi::types::GLuint,
    fbo: ffi::types::GLuint,
    stencil: ffi::types::GLuint,
    destruction_callback_sender: Sender<CleanupResource>,
}

impl Drop for ShadowBuffer {
    fn drop(&mut self) {
        let _ = self
            .destruction_callback_sender
            .send(CleanupResource::FramebufferObject(self.fbo));
        let _ = self
            .destruction_callback_sender
            .send(CleanupResource::Texture(self.texture));
        let _ = self
            .destruction_callback_sender
            .send(CleanupResource::RenderbufferObject(self.stencil));
    }
}

#[derive(Debug, Clone)]
struct GlesBuffer {
    dmabuf: WeakDmabuf,
    image: EGLImage,
    rbo: ffi::types::GLuint,
    fbo: ffi::types::GLuint,
    shadow: Option<Rc<ShadowBuffer>>,
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

#[derive(Debug)]
enum GlesTarget {
    Image {
        // GlesBuffer caches the shadow buffer and is renderer-local, so it works around the issue outlined below.
        // TODO: Ideally we would be able to share the texture between renderers with shared EGLContexts though.
        // But we definitly don't want to add user data to a dmabuf to facilitate this. Maybe use the EGLContexts userdata for storing the buffers?
        buf: GlesBuffer,
        dmabuf: Dmabuf,
    },
    Surface {
        surface: Rc<EGLSurface>,
        // TODO: Optimally we would cache this, but care needs to be taken. FBOs are context local, while Textures might be shared.
        // So we can't just put it in user-data for an `EGLSurface`, as the same surface might be used with multiple shared Contexts.
        shadow: Option<ShadowBuffer>,
    },
    Texture {
        texture: GlesTexture,
        fbo: ffi::types::GLuint,
        // TODO: Optimally we would cache this, but care needs to be taken. FBOs are context local, while Textures might be shared.
        // So we can't just store it in the GlesTexture, but need a renderer-id HashMap for the FBOs.
        shadow: Option<ShadowBuffer>,
        destruction_callback_sender: Sender<CleanupResource>,
    },
    Renderbuffer {
        buf: GlesRenderbuffer,
        fbo: ffi::types::GLuint,
        // TODO: Optimally we would cache this, but care needs to be taken. FBOs are context local, while Renderbuffers might be shared.
        // So we can't just store it in the GlesRenderbuffer, but need a renderer-id HashMap for the FBOs.
        shadow: Option<ShadowBuffer>,
    },
}

impl GlesTarget {
    fn format(&self) -> Option<(ffi::types::GLenum, bool)> {
        match self {
            GlesTarget::Image { dmabuf, .. } => {
                let format = crate::backend::allocator::Buffer::format(dmabuf).code;
                let has_alpha = has_alpha(format);
                let (format, _, _) = fourcc_to_gl_formats(if has_alpha {
                    format
                } else {
                    get_transparent(format)?
                })?;

                Some((format, has_alpha))
            }
            GlesTarget::Surface { surface, .. } => {
                let format = surface.pixel_format();
                let format = match (format.color_bits, format.alpha_bits) {
                    (24, 8) => ffi::RGB8,
                    (30, 2) => ffi::RGB10_A2,
                    (48, 16) => ffi::RGB16F,
                    _ => return None,
                };

                Some((format, true))
            }
            GlesTarget::Texture { texture, .. } => Some((texture.0.format?, texture.0.has_alpha)),
            GlesTarget::Renderbuffer { buf, .. } => Some((buf.0.format, buf.0.has_alpha)),
        }
    }

    #[profiling::function]
    fn make_current(&self, gl: &ffi::Gles2, egl: &EGLContext) -> Result<(), MakeCurrentError> {
        unsafe {
            if let GlesTarget::Surface { surface, shadow, .. } = self {
                egl.make_current_with_surface(surface)?;
                if let Some(shadow) = shadow.as_ref() {
                    gl.BindFramebuffer(ffi::FRAMEBUFFER, shadow.fbo);
                } else {
                    gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
                }
            } else {
                egl.make_current()?;
                match self {
                    GlesTarget::Image { ref buf, .. } => {
                        if let Some(shadow) = buf.shadow.as_ref() {
                            gl.BindFramebuffer(ffi::FRAMEBUFFER, shadow.fbo);
                        } else {
                            gl.BindFramebuffer(ffi::FRAMEBUFFER, buf.fbo)
                        }
                    }
                    GlesTarget::Texture {
                        ref fbo, ref shadow, ..
                    } => {
                        if let Some(shadow) = shadow.as_ref() {
                            gl.BindFramebuffer(ffi::FRAMEBUFFER, shadow.fbo);
                        } else {
                            gl.BindFramebuffer(ffi::FRAMEBUFFER, *fbo)
                        }
                    }
                    GlesTarget::Renderbuffer {
                        ref fbo, ref shadow, ..
                    } => {
                        if let Some(shadow) = shadow.as_ref() {
                            gl.BindFramebuffer(ffi::FRAMEBUFFER, shadow.fbo);
                        } else {
                            gl.BindFramebuffer(ffi::FRAMEBUFFER, *fbo)
                        }
                    }
                    _ => unreachable!(),
                }
            }
            Ok(())
        }
    }

    #[profiling::function]
    fn make_current_no_shadow(
        &self,
        gl: &ffi::Gles2,
        egl: &EGLContext,
        stencil: Option<(ffi::types::GLuint, ffi::types::GLuint)>,
    ) -> Result<(), GlesError> {
        unsafe {
            if let GlesTarget::Surface { surface, .. } = self {
                egl.make_current_with_surface(surface)?;
                let size = surface.get_size().ok_or(GlesError::UnexpectedSize)?;

                if let Some((stencil_fbo, _)) = stencil {
                    gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
                    gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, stencil_fbo);
                    gl.BlitFramebuffer(
                        0,
                        0,
                        size.w,
                        size.h,
                        0,
                        0,
                        size.w,
                        size.h,
                        ffi::STENCIL_BUFFER_BIT,
                        ffi::NEAREST,
                    );
                    gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, 0);
                }
            } else {
                egl.make_current()?;
                match self {
                    GlesTarget::Image { ref buf, .. } => {
                        gl.BindFramebuffer(ffi::FRAMEBUFFER, buf.fbo);
                    }
                    GlesTarget::Texture { ref fbo, .. } => {
                        gl.BindFramebuffer(ffi::FRAMEBUFFER, *fbo);
                    }
                    GlesTarget::Renderbuffer { ref fbo, .. } => {
                        gl.BindFramebuffer(ffi::FRAMEBUFFER, *fbo);
                    }
                    _ => unreachable!(),
                }
                if let Some((_, stencil_rbo)) = stencil {
                    gl.FramebufferRenderbuffer(
                        ffi::FRAMEBUFFER,
                        ffi::STENCIL_ATTACHMENT,
                        ffi::RENDERBUFFER,
                        stencil_rbo,
                    );
                }
            }
        }
        Ok(())
    }

    fn has_shadow(&self) -> bool {
        self.get_shadow().is_some()
    }

    fn get_shadow(&self) -> Option<&ShadowBuffer> {
        match self {
            GlesTarget::Surface { ref shadow, .. } => shadow.as_ref(),
            GlesTarget::Image { ref buf, .. } => buf.shadow.as_deref(),
            GlesTarget::Texture { ref shadow, .. } => shadow.as_ref(),
            GlesTarget::Renderbuffer { ref shadow, .. } => shadow.as_ref(),
        }
    }
}

impl Drop for GlesTarget {
    fn drop(&mut self) {
        match self {
            GlesTarget::Texture {
                fbo,
                destruction_callback_sender,
                ..
            } => {
                let _ = destruction_callback_sender.send(CleanupResource::FramebufferObject(*fbo));
            }
            GlesTarget::Renderbuffer { buf, fbo, .. } => {
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
    /// GlesRenderer supports creating of Renderbuffers with usable formats
    Renderbuffer,
    /// GlesRenderer supports color transformations
    ColorTransformations,
    /// GlesRenderer supports fencing,
    Fencing,
    /// GlesRenderer supports GL debug
    Debug,
}

/// A renderer utilizing OpenGL ES
pub struct GlesRenderer {
    // state
    target: Option<GlesTarget>,
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
    // color-transformation shaders
    // TODO new tex/solid? shaders
    output_program: Option<GlesColorOutputProgram>,

    // caches
    buffers: Vec<GlesBuffer>,
    dmabuf_cache: std::collections::HashMap<WeakDmabuf, GlesTexture>,
    vbos: [ffi::types::GLuint; 3],

    // cleanup
    destruction_callback: Receiver<CleanupResource>,
    destruction_callback_sender: Sender<CleanupResource>,

    // markers
    _not_send: *mut (),

    // debug
    span: tracing::Span,
    gl_debug_span: Option<*mut tracing::Span>,

    // profiling
    profiler: profiler::GpuProfiler,
}

/// Handle to the currently rendered frame during [`GlesRenderer::render`](Renderer::render).
///
/// Leaking this frame will prevent it from synchronizing the rendered framebuffer,
/// which might cause glitches. Additionally parts of the GL state might not be reset correctly,
/// causing unexpected results for later render commands.
/// The internal GL context and framebuffer will remain valid, no re-creation will be necessary.
pub struct GlesFrame<'frame> {
    renderer: &'frame mut GlesRenderer,
    current_projection: Matrix3<f32>,
    transform: Transform,
    size: Size<i32, Physical>,
    tex_program_override: Option<(GlesTexProgram, Vec<Uniform<'static>>)>,
    finished: AtomicBool,
    span: EnteredSpan,

    gpu_span: Option<profiler::EnteredGpuTracepoint>,
}

impl<'frame> fmt::Debug for GlesFrame<'frame> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlesFrame")
            .field("renderer", &self.renderer)
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
            .field("target", &self.target)
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
    user_param: *mut nix::libc::c_void,
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
            // required to bind the 16F-buffer we want to use for blending.
            //
            // Note: We could technically also have a 16F shadow buffer on 2.0 with GL_OES_texture_half_float.
            // The problem is, that the main output format we are interested in for this `ABGR2101010` is not renderable on 2.0,
            // and as far as I know there is no extension to change that. So we could pradoxically not copy the shadow buffer
            // to our output buffer, *except* if we scanout 16F directly...
            //
            // So lets not go down that route and attempt to support color-transformations and HDR stuff with ES 2.0.
            if exts.iter().any(|ext| ext == "GL_EXT_color_buffer_half_float") {
                capabilities.push(Capability::ColorTransformations);
                debug!("Color Transformations are supported");
            }
        }

        if exts.iter().any(|ext| ext == "GL_OES_EGL_sync") {
            debug!("Fencing is supported");
            capabilities.push(Capability::Fencing);
        }

        if exts.iter().any(|ext| ext == "GL_KHR_debug") {
            capabilities.push(Capability::Debug);
            debug!("GL Debug is supported");
        }

        Ok(capabilities)
    }

    /// Creates a new OpenGL ES renderer from a given [`EGLContext`](crate::backend::egl::EGLContext)
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

    /// Creates a new OpenGL ES renderer from a given [`EGLContext`](crate::backend::egl::EGLContext)
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
    /// `EGLContext` shared with the given one (see `EGLContext::new_shared`) and can be used on
    /// any of these renderers.
    /// - This renderer has no default framebuffer, use `Bind::bind` before rendering.
    /// - Binding a new target, while another one is already bound, will replace the current target.
    /// - Shm buffers can be released after a successful import, without the texture handle becoming invalid.
    /// - Texture filtering starts with Linear-downscaling and Linear-upscaling.
    /// - The renderer might use two-pass rendering internally to facilitate color space transformations.
    ///   As such it reserves any stencil buffer for internal use and makes no guarantee about previous framebuffer
    ///   contents being accessible during the lifetime of a `GlesFrame`.
    pub unsafe fn with_capabilities(
        context: EGLContext,
        capabilities: impl IntoIterator<Item = Capability>,
    ) -> Result<GlesRenderer, GlesError> {
        let span = info_span!(parent: &context.span, "renderer_gles2");
        let _guard = span.enter();

        context.make_current()?;

        let supported_capabilities = Self::supported_capabilities(&context)?;
        let mut requested_capabilities = capabilities.into_iter().collect::<Vec<_>>();

        // Color transform requires blit
        if requested_capabilities.contains(&Capability::ColorTransformations)
            && !requested_capabilities.contains(&Capability::Blit)
        {
            requested_capabilities.push(Capability::Blit);
        }

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
                Capability::Blit => GlesError::GLVersionNotSupported(version::GLES_3_0),
                Capability::Renderbuffer => GlesError::GLExtensionNotSupported(&["GL_OES_rgb8_rgba8"]),
                Capability::ColorTransformations => {
                    GlesError::GLExtensionNotSupported(&["GL_EXT_color_buffer_half_float"])
                }
                Capability::Fencing => GlesError::GLExtensionNotSupported(&["GL_OES_EGL_sync"]),
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
        let output_program = capabilities
            .contains(&Capability::ColorTransformations)
            .then(|| color_output_program(&gl, tx.clone()))
            .transpose()?;

        // Initialize vertices based on drawing methodology.
        let vertices: &[ffi::types::GLfloat] = if capabilities.contains(&Capability::Instancing) {
            &INSTANCED_VERTS
        } else {
            &TRIANGLE_VERTS
        };

        let mut vbos = [0; 3];
        gl.GenBuffers(vbos.len() as i32, vbos.as_mut_ptr());
        gl.BindBuffer(ffi::ARRAY_BUFFER, vbos[0]);
        gl.BufferData(
            ffi::ARRAY_BUFFER,
            std::mem::size_of_val(vertices) as isize,
            vertices.as_ptr() as *const _,
            ffi::STATIC_DRAW,
        );
        gl.BindBuffer(ffi::ARRAY_BUFFER, vbos[2]);
        gl.BufferData(
            ffi::ARRAY_BUFFER,
            (std::mem::size_of::<ffi::types::GLfloat>() * OUTPUT_VERTS.len()) as isize,
            OUTPUT_VERTS.as_ptr() as *const _,
            ffi::STATIC_DRAW,
        );
        gl.BindBuffer(ffi::ARRAY_BUFFER, 0);

        context
            .user_data()
            .insert_if_missing(|| RendererId(next_renderer_id()));

        drop(_guard);

        let profiler = profiler::GpuProfiler::new(&gl);

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
            output_program,
            vbos,
            min_filter: TextureFilter::Linear,
            max_filter: TextureFilter::Linear,

            target: None,
            buffers: Vec::new(),
            dmabuf_cache: std::collections::HashMap::new(),

            destruction_callback: rx,
            destruction_callback_sender: tx,

            debug_flags: DebugFlags::empty(),
            _not_send: std::ptr::null_mut(),
            span,
            gl_debug_span,

            profiler,
        };
        renderer.egl.unbind()?;
        Ok(renderer)
    }

    #[profiling::function]
    pub(crate) fn make_current(&mut self) -> Result<(), MakeCurrentError> {
        if let Some(target) = self.target.as_ref() {
            target.make_current(&self.gl, &self.egl)?;
        } else {
            unsafe { self.egl.make_current()? };
        }
        // delayed destruction until the next frame rendering.
        self.cleanup();
        Ok(())
    }

    #[profiling::function]
    fn cleanup(&mut self) {
        #[cfg(feature = "wayland_frontend")]
        self.dmabuf_cache.retain(|entry, _tex| entry.upgrade().is_some());
        // Free outdated buffer resources
        // TODO: Replace with `drain_filter` once it lands
        let mut i = 0;
        while i != self.buffers.len() {
            if self.buffers[i].dmabuf.is_gone() {
                let _scope = self
                    .profiler
                    .scope(tracy_client::span_location!("cleanup dmabuf"), &self.gl);
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
            let _scope = self
                .profiler
                .scope(tracy_client::span_location!("cleanup resources"), &self.gl);
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
        type CacheMap = HashMap<usize, Rc<GlesTextureInternal>>;

        with_buffer_contents(buffer, |ptr, len, data| {
            self.make_current()?;

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
            let (mut internal_format, read_format, type_) = fourcc_to_gl_formats(if has_alpha {
                fourcc
            } else {
                get_transparent(fourcc).ok_or(GlesError::UnsupportedWlPixelFormat(data.format))?
            })
            .ok_or(GlesError::UnsupportedWlPixelFormat(data.format))?;
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

            let id = self.id();
            let texture = GlesTexture(
                surface
                    .and_then(|surface| {
                        surface
                            .data_map
                            .insert_if_missing(|| Rc::new(RefCell::new(CacheMap::new())));
                        surface
                            .data_map
                            .get::<Rc<RefCell<CacheMap>>>()
                            .unwrap()
                            .borrow()
                            .get(&id)
                            .cloned()
                    })
                    .filter(|texture| texture.size == (width, height).into())
                    .unwrap_or_else(|| {
                        let mut tex = 0;
                        unsafe { self.gl.GenTextures(1, &mut tex) };
                        // new texture, upload in full
                        upload_full = true;
                        let new = Rc::new(GlesTextureInternal {
                            texture: tex,
                            format: Some(internal_format),
                            has_alpha,
                            is_external: false,
                            y_inverted: false,
                            size: (width, height).into(),
                            egl_images: None,
                            destruction_callback_sender: self.destruction_callback_sender.clone(),
                        });
                        if let Some(surface) = surface {
                            let copy = new.clone();
                            surface
                                .data_map
                                .get::<Rc<RefCell<CacheMap>>>()
                                .unwrap()
                                .borrow_mut()
                                .insert(id, copy);
                        }
                        new
                    }),
            );

            let _scope = self
                .profiler
                .scope(tracy_client::span_location!("import_shm_buffer"), &self.gl);
            unsafe {
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
            }

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
        self.make_current()?;

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
        let (mut internal, format, layout) = fourcc_to_gl_formats(if has_alpha {
            format
        } else {
            get_transparent(format).expect("We check the format before")
        })
        .expect("We check the format before");
        if self.gl_version.major == 2 {
            // es 2.0 doesn't define sized variants
            internal = match internal {
                ffi::RGBA8 => ffi::RGBA,
                ffi::RGB8 => ffi::RGB,
                _ => unreachable!(),
            };
        }

        let texture = GlesTexture(Rc::new({
            let _scope = self
                .profiler
                .scope(tracy_client::span_location!("import_memory"), &self.gl);
            let mut tex = 0;
            unsafe {
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
            // new texture, upload in full
            GlesTextureInternal {
                texture: tex,
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
        texture: &<Self as Renderer>::TextureId,
        data: &[u8],
        region: Rectangle<i32, BufferCoord>,
    ) -> Result<(), <Self as Renderer>::Error> {
        self.make_current()?;

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

        let _scope = self
            .profiler
            .scope(tracy_client::span_location!("update_memory"), &self.gl);
        unsafe {
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
        self.make_current()?;

        let egl = self
            .egl_reader
            .as_ref()
            .unwrap()
            .egl_buffer_contents(buffer)
            .map_err(GlesError::EGLBufferAccessError)?;

        let tex = self.import_egl_image(egl.image(0).unwrap(), egl.format == EGLFormat::External, None)?;

        let texture = GlesTexture(Rc::new(GlesTextureInternal {
            texture: tex,
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

        self.make_current()?;
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
            let texture = GlesTexture(Rc::new(GlesTextureInternal {
                texture: tex,
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

    fn dmabuf_formats(&self) -> Box<dyn Iterator<Item = Format>> {
        Box::new(
            self.egl
                .dmabuf_texture_formats()
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .into_iter(),
        )
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportDmaWl for GlesRenderer {}

impl GlesRenderer {
    #[profiling::function]
    fn existing_dmabuf_texture(&mut self, buffer: &Dmabuf) -> Result<Option<GlesTexture>, GlesError> {
        let existing_texture = self
            .dmabuf_cache
            .iter()
            .find(|(weak, _)| weak.upgrade().map(|entry| &entry == buffer).unwrap_or(false))
            .map(|(_, tex)| tex.clone());

        if let Some(texture) = existing_texture {
            trace!("Re-using texture {:?} for {:?}", texture.0.texture, buffer);
            if let Some(egl_images) = texture.0.egl_images.as_ref() {
                if egl_images[0] == ffi_egl::NO_IMAGE_KHR {
                    return Ok(None);
                }
                let tex = Some(texture.0.texture);
                self.import_egl_image(egl_images[0], texture.0.is_external, tex)?;
            }
            Ok(Some(texture))
        } else {
            Ok(None)
        }
    }

    #[profiling::function]
    fn import_egl_image(
        &mut self,
        image: EGLImage,
        is_external: bool,
        tex: Option<u32>,
    ) -> Result<u32, GlesError> {
        let _scope = self
            .profiler
            .scope(tracy_client::span_location!("import_egl_image"), &self.gl);
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

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn copy_framebuffer(
        &mut self,
        region: Rectangle<i32, BufferCoord>,
        fourcc: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        if let Some(target) = self.target.as_ref() {
            target.make_current_no_shadow(&self.gl, &self.egl, None)?;
        } else {
            unsafe {
                self.egl.make_current()?;
            }
        }

        let (_, has_alpha) = self
            .target
            .as_ref()
            .ok_or(GlesError::UnknownPixelFormat)?
            .format()
            .ok_or(GlesError::UnknownPixelFormat)?;
        let (_, format, layout) = fourcc_to_gl_formats(fourcc).ok_or(GlesError::UnknownPixelFormat)?;

        let _scope = self
            .profiler
            .scope(tracy_client::span_location!("copy_framebuffer"), &self.gl);
        let mut pbo = 0;
        let err = unsafe {
            self.gl.GetError(); // clear errors
            self.gl.GenBuffers(1, &mut pbo);
            self.gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, pbo);
            let bpp = gl_bpp(format, layout).ok_or(GlesError::UnsupportedPixelLayout)? / 8;
            let size = (region.size.w * region.size.h * bpp as i32) as isize;
            self.gl
                .BufferData(ffi::PIXEL_PACK_BUFFER, size, ptr::null(), ffi::STREAM_READ);
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
                has_alpha,
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
    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, BufferCoord>,
        fourcc: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        let mut pbo = 0;
        let old_target = self.target.take();
        self.bind(texture.clone())?;
        self.target
            .as_ref()
            .unwrap()
            .make_current_no_shadow(&self.gl, &self.egl, None)?;

        let (_, format, layout) = fourcc_to_gl_formats(fourcc).ok_or(GlesError::UnknownPixelFormat)?;
        let bpp = gl_bpp(format, layout).expect("We check the format before") / 8;

        let gpu_span = self
            .profiler
            .scope(tracy_client::span_location!("copy_texture"), &self.gl);
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
        std::mem::drop(gpu_span);

        // restore old framebuffer
        self.unbind()?;
        self.target = old_target;
        self.make_current()?;

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
        self.make_current()?;
        let size = texture_mapping.size();
        let len = size.w * size.h * 4;

        let mapping_ptr = texture_mapping.mapping.load(Ordering::SeqCst);
        let ptr = if mapping_ptr.is_null() {
            unsafe {
                let _scope = self
                    .profiler
                    .scope(tracy_client::span_location!("map_texture"), &self.gl);
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

impl GlesRenderer {
    #[profiling::function]
    fn create_shadow_buffer(
        &mut self,
        size: Size<i32, BufferCoord>,
    ) -> Result<Option<ShadowBuffer>, GlesError> {
        trace!(?size, "Creating shadow framebuffer");

        self.capabilities
            .contains(&Capability::ColorTransformations)
            .then(|| {
                let _scope = self
                    .profiler
                    .scope(tracy_client::span_location!("create_shadow_buffer"), &self.gl);
                let mut tex = unsafe {
                    let mut tex = 0;
                    self.gl.GenTextures(1, &mut tex);
                    self.gl.BindTexture(ffi::TEXTURE_2D, tex);
                    self.gl.TexImage2D(
                        ffi::TEXTURE_2D,
                        0,
                        ffi::RGBA16F as i32,
                        size.w,
                        size.h,
                        0,
                        ffi::RGBA,
                        ffi::HALF_FLOAT,
                        std::ptr::null(),
                    );
                    tex
                };

                let mut fbo = unsafe {
                    let mut fbo = 0;
                    self.gl.GenFramebuffers(1, &mut fbo as *mut _);
                    self.gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo);
                    self.gl.FramebufferTexture2D(
                        ffi::FRAMEBUFFER,
                        ffi::COLOR_ATTACHMENT0,
                        ffi::TEXTURE_2D,
                        tex,
                        0,
                    );
                    fbo
                };

                let stencil = unsafe {
                    let mut rbo = 0;

                    self.gl.GenRenderbuffers(1, &mut rbo);
                    self.gl.BindRenderbuffer(ffi::RENDERBUFFER, rbo);
                    self.gl
                        .RenderbufferStorage(ffi::RENDERBUFFER, ffi::STENCIL_INDEX8, size.w, size.h);

                    self.gl.FramebufferRenderbuffer(
                        ffi::FRAMEBUFFER,
                        ffi::STENCIL_ATTACHMENT,
                        ffi::RENDERBUFFER,
                        rbo,
                    );

                    let status = self.gl.CheckFramebufferStatus(ffi::FRAMEBUFFER);
                    self.gl.BindTexture(ffi::TEXTURE_2D, 0);
                    self.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
                    self.gl.BindRenderbuffer(ffi::RENDERBUFFER, 0);

                    if status != ffi::FRAMEBUFFER_COMPLETE {
                        self.gl.DeleteFramebuffers(1, &mut fbo as *mut _);
                        self.gl.DeleteTextures(1, &mut tex as *mut _);
                        self.gl.DeleteRenderbuffers(1, &mut rbo as *mut _);
                        return Err(GlesError::FramebufferBindingError);
                    }

                    rbo
                };

                Ok(ShadowBuffer {
                    texture: tex,
                    fbo,
                    stencil,
                    destruction_callback_sender: self.destruction_callback_sender.clone(),
                })
            })
            .transpose()
    }
}

impl Bind<Rc<EGLSurface>> for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn bind(&mut self, surface: Rc<EGLSurface>) -> Result<(), GlesError> {
        self.unbind()?;
        self.target = Some(GlesTarget::Surface {
            surface: surface.clone(),
            shadow: None,
        });
        self.make_current()?;

        let size = surface
            .get_size()
            .ok_or(GlesError::UnknownSize)?
            .to_logical(1)
            .to_buffer(1, Transform::Normal);
        if let Some(shadow) = self.create_shadow_buffer(size)? {
            self.target = Some(GlesTarget::Surface {
                surface,
                shadow: Some(shadow),
            });
            self.make_current()?;
        }
        Ok(())
    }
}

impl Bind<Dmabuf> for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn bind(&mut self, dmabuf: Dmabuf) -> Result<(), GlesError> {
        self.unbind()?;
        self.make_current()?;

        let (buf, dmabuf) = self
            .buffers
            .iter_mut()
            .find(|buffer| {
                if let Some(dma) = buffer.dmabuf.upgrade() {
                    dma == dmabuf
                } else {
                    false
                }
            })
            .map(|buf| Ok((buf.clone(), buf.dmabuf.upgrade().unwrap())))
            .unwrap_or_else(|| {
                trace!("Creating EGLImage for Dmabuf: {:?}", dmabuf);
                let image = self
                    .egl
                    .display()
                    .create_image_from_dmabuf(&dmabuf)
                    .map_err(GlesError::BindBufferEGLError)?;

                let scope = self
                    .profiler
                    .enter(tracy_client::span_location!("bind dmabuf"), &self.gl);
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
                        //TODO wrap image and drop here
                        return Err(GlesError::FramebufferBindingError);
                    }
                    let shadow = self.create_shadow_buffer(dmabuf.0.size)?;
                    self.profiler.exit(&self.gl, scope);

                    let buf = GlesBuffer {
                        dmabuf: dmabuf.weak(),
                        image,
                        rbo,
                        fbo,
                        shadow: shadow.map(Rc::new),
                    };

                    self.buffers.push(buf.clone());

                    Ok((buf, dmabuf))
                }
            })?;

        self.target = Some(GlesTarget::Image { buf, dmabuf });
        self.make_current()?;
        Ok(())
    }

    fn supported_formats(&self) -> Option<HashSet<Format>> {
        Some(self.egl.display().dmabuf_render_formats().clone())
    }
}

impl Bind<GlesTexture> for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn bind(&mut self, texture: GlesTexture) -> Result<(), GlesError> {
        self.unbind()?;
        self.make_current()?;

        let scope = self
            .profiler
            .enter(tracy_client::span_location!("bind texture"), &self.gl);
        let mut fbo = 0;
        unsafe {
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

        let shadow = self.create_shadow_buffer(texture.size())?;
        self.profiler.exit(&self.gl, scope);
        self.target = Some(GlesTarget::Texture {
            texture,
            shadow,
            destruction_callback_sender: self.destruction_callback_sender.clone(),
            fbo,
        });
        self.make_current()?;

        Ok(())
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
        self.make_current()?;

        let has_alpha = has_alpha(format);
        let (internal, format, layout) = fourcc_to_gl_formats(if has_alpha {
            format
        } else {
            get_transparent(format).ok_or(GlesError::UnsupportedPixelFormat(format))?
        })
        .ok_or(GlesError::UnsupportedPixelFormat(format))?;
        if (internal != ffi::RGBA8 && internal != ffi::BGRA_EXT)
            && !self.capabilities.contains(&Capability::ColorTransformations)
        {
            return Err(GlesError::UnsupportedPixelLayout);
        }

        let scope = self
            .profiler
            .scope(tracy_client::span_location!("create texture buffer"), &self.gl);
        let tex = unsafe {
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
        std::mem::drop(scope);

        Ok(unsafe { GlesTexture::from_raw(self, Some(internal), !has_alpha, tex, size) })
    }
}

impl Bind<GlesRenderbuffer> for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn bind(&mut self, renderbuffer: GlesRenderbuffer) -> Result<(), GlesError> {
        self.unbind()?;
        self.make_current()?;

        let scope = self
            .profiler
            .enter(tracy_client::span_location!("bind renderbuffer"), &self.gl);
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

        let shadow = self.create_shadow_buffer(renderbuffer.size())?;
        self.profiler.exit(&self.gl, scope);
        self.target = Some(GlesTarget::Renderbuffer {
            buf: renderbuffer,
            shadow,
            fbo,
        });
        self.make_current()?;

        Ok(())
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
        self.make_current()?;

        let has_alpha = has_alpha(format);
        let (internal, _, _) = fourcc_to_gl_formats(if has_alpha {
            format
        } else {
            get_transparent(format).ok_or(GlesError::UnsupportedPixelFormat(format))?
        })
        .ok_or(GlesError::UnsupportedPixelFormat(format))?;

        if internal != ffi::RGBA8 && !self.capabilities.contains(&Capability::ColorTransformations) {
            return Err(GlesError::UnsupportedPixelLayout);
        }

        let _scope = self.profiler.scope(
            tracy_client::span_location!("create renderbuffer buffer"),
            &self.gl,
        );
        unsafe {
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

impl<Target> Blit<Target> for GlesRenderer
where
    Self: Bind<Target>,
{
    #[instrument(level = "trace", parent = &self.span, skip(self, to))]
    #[profiling::function]
    fn blit_to(
        &mut self,
        to: Target,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), GlesError> {
        let src_target = self.target.take().ok_or(GlesError::BlitError)?;
        self.bind(to)?;
        let dst_target = self.target.take().unwrap();
        self.unbind()?;

        let result = self.blit(&src_target, &dst_target, src, dst, filter);

        self.target = Some(src_target);
        self.make_current()?;

        result
    }

    #[instrument(level = "trace", parent = &self.span, skip(self, from))]
    #[profiling::function]
    fn blit_from(
        &mut self,
        from: Target,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), GlesError> {
        let dst_target = self.target.take().ok_or(GlesError::BlitError)?;
        self.bind(from)?;
        let src_target = self.target.take().unwrap();
        self.unbind()?;

        let result = self.blit(&src_target, &dst_target, src, dst, filter);

        self.unbind()?;
        self.target = Some(dst_target);
        self.make_current()?;

        result
    }
}

impl GlesRenderer {
    #[profiling::function]
    fn blit(
        &mut self,
        src_target: &GlesTarget,
        dst_target: &GlesTarget,
        src: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        filter: TextureFilter,
    ) -> Result<(), GlesError> {
        // glBlitFramebuffer is sadly only available for GLES 3.0 and higher
        if self.gl_version < version::GLES_3_0 {
            return Err(GlesError::GLVersionNotSupported(version::GLES_3_0));
        }

        match (src_target, dst_target) {
            (GlesTarget::Surface { surface: src, .. }, GlesTarget::Surface { surface: dst, .. }) => unsafe {
                self.egl.make_current_with_draw_and_read_surface(dst, src)?;
            },
            (GlesTarget::Surface { surface: src, .. }, _) => unsafe {
                self.egl.make_current_with_surface(src)?;
            },
            (_, GlesTarget::Surface { surface: dst, .. }) => unsafe {
                self.egl.make_current_with_surface(dst)?;
            },
            (_, _) => unsafe {
                self.egl.make_current()?;
            },
        }

        let scope = self
            .profiler
            .scope(tracy_client::span_location!("blit"), &self.gl);
        match src_target {
            GlesTarget::Image { ref buf, .. } => unsafe {
                self.gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, buf.fbo)
            },
            GlesTarget::Texture { ref fbo, .. } => unsafe {
                self.gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, *fbo)
            },
            GlesTarget::Renderbuffer { ref fbo, .. } => unsafe {
                self.gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, *fbo)
            },
            _ => {} // Note: The only target missing is `Surface` and handled above
        }
        match dst_target {
            GlesTarget::Image { ref buf, .. } => unsafe {
                self.gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, buf.fbo)
            },
            GlesTarget::Texture { ref fbo, .. } => unsafe {
                self.gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, *fbo)
            },
            GlesTarget::Renderbuffer { ref fbo, .. } => unsafe {
                self.gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, *fbo)
            },
            _ => {} // Note: The only target missing is `Surface` and handled above
        }

        let status = unsafe { self.gl.CheckFramebufferStatus(ffi::FRAMEBUFFER) };
        if status != ffi::FRAMEBUFFER_COMPLETE {
            std::mem::drop(scope);
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

impl Unbind for GlesRenderer {
    #[profiling::function]
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error> {
        unsafe {
            self.egl.make_current()?;
        }
        let scope = self
            .profiler
            .scope(tracy_client::span_location!("unbind"), &self.gl);
        unsafe { self.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0) };
        self.target = None;
        std::mem::drop(scope);
        self.egl.unbind()?;
        Ok(())
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
        // Don't expose the shadow buffer outside of a frame
        if let Some(target) = self.target.as_ref() {
            target.make_current_no_shadow(&self.gl, &self.egl, None)?;
        } else {
            unsafe { self.egl.make_current()? };
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
        self.make_current()?;

        let _scope = self.profiler.scope(
            tracy_client::span_location!("compile_custom_pixel_shader"),
            &self.gl,
        );
        let shader = format!("#version 100\n{}", src.as_ref());
        let program = unsafe { link_program(&self.gl, shaders::VERTEX_SHADER, &shader)? };
        let debug_shader = format!("#version 100\n#define {}\n{}", shaders::DEBUG_FLAGS, src.as_ref());
        let debug_program = unsafe { link_program(&self.gl, shaders::VERTEX_SHADER, &debug_shader)? };

        let vert = CStr::from_bytes_with_nul(b"vert\0").expect("NULL terminated");
        let vert_position = CStr::from_bytes_with_nul(b"vert_position\0").expect("NULL terminated");
        let matrix = CStr::from_bytes_with_nul(b"matrix\0").expect("NULL terminated");
        let tex_matrix = CStr::from_bytes_with_nul(b"tex_matrix\0").expect("NULL terminated");
        let size = CStr::from_bytes_with_nul(b"size\0").expect("NULL terminated");
        let alpha = CStr::from_bytes_with_nul(b"alpha\0").expect("NULL terminated");
        let tint = CStr::from_bytes_with_nul(b"tint\0").expect("NULL terminated");

        unsafe {
            Ok(GlesPixelProgram(Rc::new(GlesPixelProgramInner {
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
        self.make_current()?;

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

impl<'frame> GlesFrame<'frame> {
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

impl Renderer for GlesRenderer {
    type Error = GlesError;
    type TextureId = GlesTexture;
    type Frame<'frame> = GlesFrame<'frame>;

    fn id(&self) -> usize {
        self.egl.user_data().get::<RendererId>().unwrap().0
    }

    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.min_filter = filter;
        Ok(())
    }
    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.max_filter = filter;
        Ok(())
    }

    #[profiling::function]
    fn render(
        &mut self,
        mut output_size: Size<i32, Physical>,
        transform: Transform,
    ) -> Result<GlesFrame<'_>, Self::Error> {
        self.make_current()?;
        self.profiler.collect(&self.gl);
        let gpu_span = self
            .profiler
            .enter(tracy_client::span_location!("render"), &self.gl);
        let scope = self
            .profiler
            .enter(tracy_client::span_location!("setup"), &self.gl);

        unsafe {
            self.gl.Viewport(0, 0, output_size.w, output_size.h);

            self.gl.Scissor(0, 0, output_size.w, output_size.h);
            self.gl.Enable(ffi::SCISSOR_TEST);

            self.gl.Enable(ffi::BLEND);
            self.gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);

            if self
                .target
                .as_ref()
                .map(|target| target.has_shadow())
                .unwrap_or(false)
            {
                let _scope = self
                    .profiler
                    .scope(tracy_client::span_location!("setup shadow buffer"), &self.gl);
                // Enable stencil testing and clear the shadow buffer for blending onto the actual framebuffer in finish
                self.gl.Enable(ffi::STENCIL_TEST);
                self.gl.StencilFunc(ffi::ALWAYS, 1, ffi::types::GLuint::MAX);
                self.gl.StencilOp(ffi::REPLACE, ffi::REPLACE, ffi::REPLACE);
                self.gl.StencilMask(ffi::types::GLuint::MAX);
                self.gl.Clear(ffi::COLOR_BUFFER_BIT | ffi::STENCIL_BUFFER_BIT);
            }
        }
        self.profiler.exit(&self.gl, scope);

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

    fn set_debug_flags(&mut self, flags: DebugFlags) {
        self.debug_flags = flags;
    }

    fn debug_flags(&self) -> DebugFlags {
        self.debug_flags
    }

    #[profiling::function]
    fn wait(&mut self, sync: &super::sync::SyncPoint) -> Result<(), Self::Error> {
        self.make_current()?;

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
        sync.wait();
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

impl<'frame> Frame for GlesFrame<'frame> {
    type TextureId = GlesTexture;
    type Error = GlesError;

    fn id(&self) -> usize {
        self.renderer.id()
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    fn clear(&mut self, color: [f32; 4], at: &[Rectangle<i32, Physical>]) -> Result<(), GlesError> {
        if at.is_empty() {
            return Ok(());
        }

        let scope = self
            .renderer
            .profiler
            .enter(tracy_client::span_location!("clear"), &self.renderer.gl);
        unsafe {
            self.renderer.gl.Disable(ffi::BLEND);
        }

        let res = self.draw_solid(Rectangle::from_loc_and_size((0, 0), self.size), at, color);

        unsafe {
            self.renderer.gl.Enable(ffi::BLEND);
            self.renderer.gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
        }
        self.renderer.profiler.exit(&self.renderer.gl, scope);

        res
    }

    #[instrument(level = "trace", skip(self), parent = &self.span)]
    #[profiling::function]
    fn draw_solid(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: [f32; 4],
    ) -> Result<(), Self::Error> {
        if damage.is_empty() {
            return Ok(());
        }

        let is_opaque = color[3] == 1f32;

        let scope = self
            .renderer
            .profiler
            .enter(tracy_client::span_location!("draw_solid"), &self.renderer.gl);
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
        self.renderer.profiler.exit(&self.renderer.gl, scope);

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
        transform: Transform,
        alpha: f32,
    ) -> Result<(), GlesError> {
        self.render_texture_from_to(texture, src, dest, damage, transform, alpha, None, &[])
    }

    fn transformation(&self) -> Transform {
        self.transform
    }

    #[profiling::function]
    fn finish(mut self) -> Result<SyncPoint, Self::Error> {
        self.finish_internal()
    }

    #[profiling::function]
    fn wait(&mut self, sync: &SyncPoint) -> Result<(), Self::Error> {
        self.renderer.wait(sync)
    }
}

impl<'frame> GlesFrame<'frame> {
    #[profiling::function]
    fn finish_internal(&mut self) -> Result<SyncPoint, GlesError> {
        let _guard = self.span.enter();

        if self.finished.swap(true, Ordering::SeqCst) {
            return Ok(SyncPoint::signaled());
        }

        let finish_gpu_span = self
            .renderer
            .profiler
            .enter(tracy_client::span_location!("finish_internal"), &self.renderer.gl);
        unsafe {
            self.renderer.gl.Disable(ffi::SCISSOR_TEST);
            self.renderer.gl.Disable(ffi::BLEND);
        }

        if let Some(target) = self.renderer.target.as_ref() {
            if let Some(shadow) = target.get_shadow() {
                target.make_current_no_shadow(
                    &self.renderer.gl,
                    &self.renderer.egl,
                    Some((shadow.fbo, shadow.stencil)),
                )?;
                let _scope = self.renderer.profiler.scope(
                    tracy_client::span_location!("draw shadow buffer"),
                    &self.renderer.gl,
                );
                unsafe {
                    self.renderer
                        .gl
                        .StencilFunc(ffi::NOTEQUAL, 0, ffi::types::GLuint::MAX);
                    self.renderer.gl.StencilMask(0);

                    self.renderer.gl.ActiveTexture(ffi::TEXTURE0);
                    self.renderer.gl.BindTexture(ffi::TEXTURE_2D, shadow.texture);
                    self.renderer.gl.TexParameteri(
                        ffi::TEXTURE_2D,
                        ffi::TEXTURE_MIN_FILTER,
                        ffi::NEAREST as i32,
                    );
                    self.renderer.gl.TexParameteri(
                        ffi::TEXTURE_2D,
                        ffi::TEXTURE_MAG_FILTER,
                        ffi::NEAREST as i32,
                    );

                    let program = self
                        .renderer
                        .output_program
                        .as_ref()
                        .expect("If we have a shadow buffer we have an output shader");
                    self.renderer.gl.UseProgram(program.program);
                    self.renderer.gl.Uniform1i(program.uniform_tex, 0);

                    self.renderer
                        .gl
                        .EnableVertexAttribArray(program.attrib_vert as u32);
                    self.renderer
                        .gl
                        .BindBuffer(ffi::ARRAY_BUFFER, self.renderer.vbos[2]);
                    self.renderer.gl.VertexAttribPointer(
                        program.attrib_vert as u32,
                        2,
                        ffi::FLOAT,
                        ffi::FALSE,
                        0,
                        std::ptr::null(),
                    );

                    self.renderer.gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);

                    self.renderer.gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
                    self.renderer.gl.DisableVertexAttribArray(0);
                    self.renderer.gl.Disable(ffi::STENCIL_TEST);
                }
            }
        }

        self.renderer.profiler.exit(&self.renderer.gl, finish_gpu_span);
        if let Some(span) = self.gpu_span.take() {
            self.renderer.profiler.exit(&self.renderer.gl, span);
        }

        unsafe {
            self.renderer.gl.Flush();
        }

        // if we support egl fences we should use it
        if self.renderer.capabilities.contains(&Capability::Fencing) {
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
    #[instrument(skip(self), parent = &self.span)]
    #[profiling::function]
    pub fn draw_solid(
        &mut self,
        dest: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: [f32; 4],
    ) -> Result<(), GlesError> {
        if damage.is_empty() {
            return Ok(());
        }

        let mut mat = Matrix3::<f32>::identity();
        mat = self.current_projection * mat;

        let instances = damage
            .iter()
            .flat_map(|rect| {
                let dest_size = dest.size;

                let rect_constrained_loc = rect
                    .loc
                    .constrain(Rectangle::from_extemities((0, 0), dest_size.to_point()));
                let rect_clamped_size = rect
                    .size
                    .clamp((0, 0), (dest_size.to_point() - rect_constrained_loc).to_size());

                let rect = Rectangle::from_loc_and_size(rect_constrained_loc, rect_clamped_size);
                [
                    (dest.loc.x + rect.loc.x) as f32,
                    (dest.loc.y + rect.loc.y) as f32,
                    rect.size.w as f32,
                    rect.size.h as f32,
                ]
            })
            .collect::<Vec<_>>();

        let gl = &self.renderer.gl;
        let _scope = self
            .renderer
            .profiler
            .scope(tracy_client::span_location!("draw_solid"), &self.renderer.gl);
        unsafe {
            gl.UseProgram(self.renderer.solid_program.program);
            gl.Uniform4f(
                self.renderer.solid_program.uniform_color,
                color[0],
                color[1],
                color[2],
                color[3],
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

            // Damage vertices.
            let vertices = if self.renderer.capabilities.contains(&Capability::Instancing) {
                instances
            } else {
                // Add the 4 f32s per damage rectangle for each of the 6 vertices.
                let mut vertices = Vec::with_capacity(instances.len() * 6);
                for chunk in instances.chunks(4) {
                    for _ in 0..6 {
                        vertices.extend_from_slice(chunk);
                    }
                }
                vertices
            };

            gl.EnableVertexAttribArray(self.renderer.solid_program.attrib_position as u32);
            gl.BindBuffer(ffi::ARRAY_BUFFER, self.renderer.vbos[1]);
            gl.BufferData(
                ffi::ARRAY_BUFFER,
                (std::mem::size_of::<ffi::types::GLfloat>() * vertices.len()) as isize,
                vertices.as_ptr() as *const _,
                ffi::STREAM_DRAW,
            );

            gl.VertexAttribPointer(
                self.renderer.solid_program.attrib_position as u32,
                4,
                ffi::FLOAT,
                ffi::FALSE,
                0,
                std::ptr::null(),
            );

            let damage_len = damage.len() as i32;
            if self.renderer.capabilities.contains(&Capability::Instancing) {
                gl.VertexAttribDivisor(self.renderer.solid_program.attrib_vert as u32, 0);

                gl.VertexAttribDivisor(self.renderer.solid_program.attrib_position as u32, 1);

                gl.DrawArraysInstanced(ffi::TRIANGLE_STRIP, 0, 4, damage_len);
            } else {
                // When we have more than 10 rectangles, draw them in batches of 10.
                for i in 0..(damage_len - 1) / 10 {
                    gl.DrawArrays(ffi::TRIANGLES, 0, 60);

                    // Set damage pointer to the next 10 rectangles.
                    let offset = (i + 1) as usize * 60 * 4 * std::mem::size_of::<ffi::types::GLfloat>();
                    gl.VertexAttribPointer(
                        self.renderer.solid_program.attrib_position as u32,
                        4,
                        ffi::FLOAT,
                        ffi::FALSE,
                        0,
                        offset as *const _,
                    );
                }

                // Draw the up to 10 remaining rectangles.
                let count = ((damage_len - 1) % 10 + 1) * 6;
                gl.DrawArrays(ffi::TRIANGLES, 0, count);
            }

            gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
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
        transform: Transform,
        alpha: f32,
        program: Option<&GlesTexProgram>,
        additional_uniforms: &[Uniform<'_>],
    ) -> Result<(), GlesError> {
        let mut mat = Matrix3::<f32>::identity();

        // dest position and scale
        mat = mat * Matrix3::from_translation(Vector2::new(dest.loc.x as f32, dest.loc.y as f32));

        // src scale, position, tranform and y_inverted
        let tex_size = texture.size().to_f64();
        let src_size = src.size;

        let transform_mat = if transform.flipped() {
            transform.invert().matrix()
        } else {
            transform.matrix()
        };

        if src_size.is_empty() || tex_size.is_empty() {
            return Ok(());
        }

        let mut tex_mat = Matrix3::<f32>::identity();
        // first scale to meet the src size
        tex_mat = tex_mat
            * Matrix3::from_nonuniform_scale(
                (src_size.w / tex_size.w) as f32,
                (src_size.h / tex_size.h) as f32,
            );
        // now translate by the src location
        tex_mat = tex_mat
            * Matrix3::from_translation(Vector2::new(
                (src.loc.x / src_size.w) as f32,
                (src.loc.y / src_size.h) as f32,
            ));
        // then apply the transform and if necessary invert the y axis
        tex_mat = tex_mat * Matrix3::from_translation(Vector2::new(0.5, 0.5));
        if transform == Transform::Normal {
            assert_eq!(tex_mat, tex_mat * transform.invert().matrix());
            assert_eq!(transform.matrix(), Matrix3::<f32>::identity());
        }
        tex_mat = tex_mat * transform_mat;
        if texture.0.y_inverted {
            tex_mat = tex_mat * Matrix3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0);
        }
        tex_mat = tex_mat * Matrix3::from_translation(Vector2::new(-0.5, -0.5));
        // at last scale back to tex space
        tex_mat = tex_mat
            * Matrix3::from_nonuniform_scale(
                (1.0f64 / dest.size.w as f64) as f32,
                (1.0f64 / dest.size.h as f64) as f32,
            );

        let instances = damage
            .iter()
            .flat_map(|rect| {
                let dest_size = dest.size;

                let rect_constrained_loc = rect
                    .loc
                    .constrain(Rectangle::from_extemities((0, 0), dest_size.to_point()));
                let rect_clamped_size = rect
                    .size
                    .clamp((0, 0), (dest_size.to_point() - rect_constrained_loc).to_size());

                let rect = Rectangle::from_loc_and_size(rect_constrained_loc, rect_clamped_size);
                [
                    rect.loc.x as f32,
                    rect.loc.y as f32,
                    rect.size.w as f32,
                    rect.size.h as f32,
                ]
            })
            .collect::<Vec<_>>();

        self.render_texture(
            texture,
            tex_mat,
            mat,
            Some(&instances),
            alpha,
            program,
            additional_uniforms,
        )
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
    #[instrument(level = "trace", skip(self), parent = &self.span)]
    #[profiling::function]
    #[allow(clippy::too_many_arguments)]
    pub fn render_texture(
        &mut self,
        tex: &GlesTexture,
        tex_matrix: Matrix3<f32>,
        mut matrix: Matrix3<f32>,
        instances: Option<&[ffi::types::GLfloat]>,
        alpha: f32,
        program: Option<&GlesTexProgram>,
        additional_uniforms: &[Uniform<'_>],
    ) -> Result<(), GlesError> {
        let damage = instances.unwrap_or(&[0.0, 0.0, 1.0, 1.0]);
        if damage.is_empty() {
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
        let scope = self
            .renderer
            .profiler
            .enter(tracy_client::span_location!("render_texture"), gl);
        unsafe {
            let scope = self
                .renderer
                .profiler
                .enter(tracy_client::span_location!("setup"), gl);
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

            // Damage vertices.
            let vertices = if self.renderer.capabilities.contains(&Capability::Instancing) {
                Cow::Borrowed(damage)
            } else {
                let mut vertices = Vec::with_capacity(damage.len() * 6);
                // Add the 4 f32s per damage rectangle for each of the 6 vertices.
                for chunk in damage.chunks(4) {
                    for _ in 0..6 {
                        vertices.extend_from_slice(chunk);
                    }
                }
                Cow::Owned(vertices)
            };

            // vert_position
            gl.EnableVertexAttribArray(program.attrib_vert_position as u32);
            gl.BindBuffer(ffi::ARRAY_BUFFER, self.renderer.vbos[1]);
            gl.BufferData(
                ffi::ARRAY_BUFFER,
                (std::mem::size_of::<ffi::types::GLfloat>() * vertices.len()) as isize,
                vertices.as_ptr() as *const _,
                ffi::STREAM_DRAW,
            );

            gl.VertexAttribPointer(
                program.attrib_vert_position as u32,
                4,
                ffi::FLOAT,
                ffi::FALSE,
                0,
                std::ptr::null(),
            );
            self.renderer.profiler.exit(gl, scope);

            let damage_len = (damage.len() / 4) as i32;
            if self.renderer.capabilities.contains(&Capability::Instancing) {
                let scope = self
                    .renderer
                    .profiler
                    .enter(tracy_client::span_location!("draw instanced"), gl);
                gl.VertexAttribDivisor(program.attrib_vert as u32, 0);
                gl.VertexAttribDivisor(program.attrib_vert_position as u32, 1);

                gl.DrawArraysInstanced(ffi::TRIANGLE_STRIP, 0, 4, damage_len);
                self.renderer.profiler.exit(gl, scope);
            } else {
                let scope = self
                    .renderer
                    .profiler
                    .enter(tracy_client::span_location!("draw batched"), gl);
                // When we have more than 10 rectangles, draw them in batches of 10.
                for i in 0..(damage_len - 1) / 10 {
                    gl.DrawArrays(ffi::TRIANGLES, 0, 6);

                    // Set damage pointer to the next 10 rectangles.
                    let offset = (i + 1) as usize * 6 * 4 * std::mem::size_of::<ffi::types::GLfloat>();
                    gl.VertexAttribPointer(
                        program.attrib_vert_position as u32,
                        4,
                        ffi::FLOAT,
                        ffi::FALSE,
                        0,
                        offset as *const _,
                    );
                }

                // Draw the up to 10 remaining rectangles.
                let count = ((damage_len - 1) % 10 + 1) * 6;
                gl.DrawArrays(ffi::TRIANGLES, 0, count);
                self.renderer.profiler.exit(gl, scope);
            }

            let scope = self
                .renderer
                .profiler
                .enter(tracy_client::span_location!("cleanup"), gl);
            gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
            gl.BindTexture(target, 0);
            gl.DisableVertexAttribArray(program.attrib_vert as u32);
            gl.DisableVertexAttribArray(program.attrib_vert_position as u32);
            self.renderer.profiler.exit(gl, scope);
        }
        self.renderer.profiler.exit(gl, scope);

        Ok(())
    }

    /// Render a pixel shader into the current target at a given `dest`-region.
    #[profiling::function]
    pub fn render_pixel_shader_to(
        &mut self,
        pixel_shader: &GlesPixelProgram,
        dest: Rectangle<i32, Physical>,
        damage: Option<&[Rectangle<i32, Physical>]>,
        alpha: f32,
        additional_uniforms: &[Uniform<'_>],
    ) -> Result<(), GlesError> {
        let damage = damage
            .map(|damage| {
                damage
                    .iter()
                    .flat_map(|rect| {
                        let dest_size = dest.size;

                        let rect_constrained_loc = rect
                            .loc
                            .constrain(Rectangle::from_extemities((0, 0), dest_size.to_point()));
                        let rect_clamped_size = rect
                            .size
                            .clamp((0, 0), (dest_size.to_point() - rect_constrained_loc).to_size());

                        let rect = Rectangle::from_loc_and_size(rect_constrained_loc, rect_clamped_size);
                        [
                            rect.loc.x as f32,
                            rect.loc.y as f32,
                            rect.size.w as f32,
                            rect.size.h as f32,
                        ]
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![0.0, 0.0, 1.0, 1.0]);

        if damage.is_empty() {
            return Ok(());
        }

        let mut matrix = Matrix3::<f32>::identity();
        let mut tex_matrix = Matrix3::<f32>::identity();

        // dest position and scale
        matrix = matrix * Matrix3::from_translation(Vector2::new(dest.loc.x as f32, dest.loc.y as f32));
        tex_matrix = tex_matrix
            * Matrix3::from_nonuniform_scale(
                (1.0f64 / dest.size.w as f64) as f32,
                (1.0f64 / dest.size.h as f64) as f32,
            );

        //apply output transformation
        matrix = self.current_projection * matrix;

        let program = if self.renderer.debug_flags.is_empty() {
            &pixel_shader.0.normal
        } else {
            &pixel_shader.0.debug
        };

        // render
        let gl = &self.renderer.gl;
        let _scope = self
            .renderer
            .profiler
            .scope(tracy_client::span_location!("render_pixel_shader_to"), gl);
        unsafe {
            gl.UseProgram(program.program);

            gl.UniformMatrix3fv(program.uniform_matrix, 1, ffi::FALSE, matrix.as_ptr());
            gl.UniformMatrix3fv(program.uniform_tex_matrix, 1, ffi::FALSE, tex_matrix.as_ptr());
            gl.Uniform2f(program.uniform_size, dest.size.w as f32, dest.size.h as f32);
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

            // Damage vertices.
            let vertices = if self.renderer.capabilities.contains(&Capability::Instancing) {
                Cow::Borrowed(&damage)
            } else {
                let mut vertices = Vec::with_capacity(damage.len() * 6);
                // Add the 4 f32s per damage rectangle for each of the 6 vertices.
                for chunk in damage.chunks(4) {
                    for _ in 0..6 {
                        vertices.extend_from_slice(chunk);
                    }
                }
                Cow::Owned(vertices)
            };

            // vert_position
            gl.EnableVertexAttribArray(program.attrib_position as u32);
            gl.BindBuffer(ffi::ARRAY_BUFFER, self.renderer.vbos[1]);
            gl.BufferData(
                ffi::ARRAY_BUFFER,
                (std::mem::size_of::<ffi::types::GLfloat>() * vertices.len()) as isize,
                vertices.as_ptr() as *const _,
                ffi::STREAM_DRAW,
            );

            gl.VertexAttribPointer(
                program.attrib_position as u32,
                4,
                ffi::FLOAT,
                ffi::FALSE,
                0,
                std::ptr::null(),
            );

            let damage_len = (damage.len() / 4) as i32;
            if self.renderer.capabilities.contains(&Capability::Instancing) {
                gl.VertexAttribDivisor(program.attrib_vert as u32, 0);
                gl.VertexAttribDivisor(program.attrib_position as u32, 1);

                gl.DrawArraysInstanced(ffi::TRIANGLE_STRIP, 0, 4, damage_len);
            } else {
                // When we have more than 10 rectangles, draw them in batches of 10.
                for i in 0..(damage_len - 1) / 10 {
                    gl.DrawArrays(ffi::TRIANGLES, 0, 6);

                    // Set damage pointer to the next 10 rectangles.
                    let offset = (i + 1) as usize * 6 * 4 * std::mem::size_of::<ffi::types::GLfloat>();
                    gl.VertexAttribPointer(
                        program.attrib_position as u32,
                        4,
                        ffi::FLOAT,
                        ffi::FALSE,
                        0,
                        offset as *const _,
                    );
                }

                // Draw the up to 10 remaining rectangles.
                let count = ((damage_len - 1) % 10 + 1) * 6;
                gl.DrawArrays(ffi::TRIANGLES, 0, count);
            }

            gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
            gl.DisableVertexAttribArray(program.attrib_vert as u32);
            gl.DisableVertexAttribArray(program.attrib_position as u32);
        }

        Ok(())
    }

    /// Projection matrix for this frame
    pub fn projection(&self) -> &[f32; 9] {
        self.current_projection.as_ref()
    }
}

impl<'frame> Drop for GlesFrame<'frame> {
    fn drop(&mut self) {
        match self.finish_internal() {
            Ok(sync) => {
                sync.wait();
            }
            Err(err) => {
                warn!("Ignored error finishing GlesFrame on drop: {}", err);
            }
        }
    }
}
