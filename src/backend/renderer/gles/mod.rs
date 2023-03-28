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
mod shaders;
mod uniform;
mod version;

pub use uniform::*;

use super::{
    Bind, Blit, DebugFlags, ExportDma, ExportMem, Frame, ImportDma, ImportMem, Offscreen, Renderer, Texture,
    TextureFilter, TextureMapping, Unbind,
};
use crate::backend::allocator::{
    dmabuf::{Dmabuf, WeakDmabuf},
    Format, Fourcc,
};
use crate::backend::egl::{
    ffi::egl::{self as ffi_egl, types::EGLImage},
    EGLContext, EGLSurface, MakeCurrentError,
};
use crate::backend::SwapBuffersError;
use crate::utils::{Buffer as BufferCoord, Physical, Rectangle, Size, Transform};

#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use super::ImportEgl;
#[cfg(feature = "wayland_frontend")]
use super::{ImportDmaWl, ImportMemWl};
#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use crate::backend::egl::{display::EGLBufferReader, Format as EGLFormat};
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_buffer, wl_shm};

#[allow(clippy::all, missing_docs, missing_debug_implementations)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/gl_bindings.rs"));
}

crate::utils::ids::id_gen!(next_renderer_id, RENDERER_ID, RENDERER_IDS);

#[derive(Debug)]
struct GlesTexProgramInternal {
    program: ffi::types::GLuint,
    uniform_tex: ffi::types::GLint,
    uniform_tex_matrix: ffi::types::GLint,
    uniform_matrix: ffi::types::GLint,
    uniform_alpha: ffi::types::GLint,
    attrib_vert: ffi::types::GLint,
    attrib_vert_position: ffi::types::GLint,
    additional_uniforms: HashMap<String, UniformDesc>,
}

#[derive(Debug)]
struct GlesTexProgramVariant {
    normal: GlesTexProgramInternal,
    debug: GlesTexProgramInternal,

    // debug flags
    uniform_tint: ffi::types::GLint,
}

/// Gles texture shader
#[derive(Debug, Clone)]
pub struct GlesTexProgram(Rc<GlesTexProgramInner>);

#[derive(Debug)]
struct GlesTexProgramInner {
    variants: [GlesTexProgramVariant; 3],
    destruction_callback_sender: Sender<CleanupResource>,
}

impl GlesTexProgram {
    fn variant_for_format(
        &self,
        format: Option<ffi::types::GLenum>,
        has_alpha: bool,
    ) -> &GlesTexProgramVariant {
        match format {
            Some(ffi::RGBA) => {
                if has_alpha {
                    &self.0.variants[0]
                } else {
                    &self.0.variants[1]
                }
            }
            None => &self.0.variants[2],
            _ => panic!("Unknown texture type"),
        }
    }
}

impl Drop for GlesTexProgramInner {
    fn drop(&mut self) {
        for variant in &self.variants {
            let _ = self
                .destruction_callback_sender
                .send(CleanupResource::Program(variant.normal.program));
            let _ = self
                .destruction_callback_sender
                .send(CleanupResource::Program(variant.debug.program));
        }
    }
}

#[derive(Debug, Clone)]
struct GlesSolidProgram {
    program: ffi::types::GLuint,
    uniform_matrix: ffi::types::GLint,
    uniform_color: ffi::types::GLint,
    attrib_vert: ffi::types::GLint,
    attrib_position: ffi::types::GLint,
}

/// Gles pixel shader
#[derive(Debug, Clone)]
pub struct GlesPixelProgram(Rc<GlesPixelProgramInner>);

#[derive(Debug)]
struct GlesPixelProgramInner {
    normal: GlesPixelProgramInternal,
    debug: GlesPixelProgramInternal,
    destruction_callback_sender: Sender<CleanupResource>,

    // debug flags
    uniform_tint: ffi::types::GLint,
}

#[derive(Debug)]
struct GlesPixelProgramInternal {
    program: ffi::types::GLuint,
    uniform_matrix: ffi::types::GLint,
    uniform_tex_matrix: ffi::types::GLint,
    uniform_size: ffi::types::GLint,
    uniform_alpha: ffi::types::GLint,
    attrib_vert: ffi::types::GLint,
    attrib_position: ffi::types::GLint,
    additional_uniforms: HashMap<String, UniformDesc>,
}

impl Drop for GlesPixelProgramInner {
    fn drop(&mut self) {
        let _ = self
            .destruction_callback_sender
            .send(CleanupResource::Program(self.normal.program));
        let _ = self
            .destruction_callback_sender
            .send(CleanupResource::Program(self.debug.program));
    }
}

/// A handle to a GLES texture
#[derive(Debug, Clone)]
pub struct GlesTexture(Rc<GlesTextureInternal>);

impl GlesTexture {
    /// Create a GlesTexture from a raw gl texture id.
    ///
    /// This expects the texture to be in RGBA format to be rendered
    /// correctly by the `render_texture*`-functions of [`Frame`](super::Frame).
    /// It is also expected to not be external or y_inverted.
    ///
    /// Ownership over the texture is taken by the renderer, you should not free the texture yourself.
    ///
    /// # Safety
    ///
    /// The renderer cannot make sure `tex` is a valid texture id.
    pub unsafe fn from_raw(
        renderer: &GlesRenderer,
        internal_format: Option<ffi::types::GLenum>,
        opaque: bool,
        tex: ffi::types::GLuint,
        size: Size<i32, BufferCoord>,
    ) -> GlesTexture {
        GlesTexture(Rc::new(GlesTextureInternal {
            texture: tex,
            format: internal_format,
            has_alpha: !opaque,
            is_external: false,
            y_inverted: false,
            size,
            egl_images: None,
            destruction_callback_sender: renderer.destruction_callback_sender.clone(),
        }))
    }

    /// OpenGL texture id of this texture
    ///
    /// This id will become invalid, when the GlesTexture is dropped and does not transfer ownership.
    pub fn tex_id(&self) -> ffi::types::GLuint {
        self.0.texture
    }
}

#[derive(Debug)]
struct GlesTextureInternal {
    texture: ffi::types::GLuint,
    format: Option<ffi::types::GLenum>,
    has_alpha: bool,
    is_external: bool,
    y_inverted: bool,
    size: Size<i32, BufferCoord>,
    egl_images: Option<Vec<EGLImage>>,
    destruction_callback_sender: Sender<CleanupResource>,
}

impl Drop for GlesTextureInternal {
    fn drop(&mut self) {
        let _ = self
            .destruction_callback_sender
            .send(CleanupResource::Texture(self.texture));
        if let Some(images) = self.egl_images.take() {
            for image in images {
                let _ = self
                    .destruction_callback_sender
                    .send(CleanupResource::EGLImage(image));
            }
        }
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

impl Texture for GlesTexture {
    fn width(&self) -> u32 {
        self.0.size.w as u32
    }
    fn height(&self) -> u32 {
        self.0.size.h as u32
    }
    fn size(&self) -> Size<i32, BufferCoord> {
        self.0.size
    }
    fn format(&self) -> Option<Fourcc> {
        let fmt = gl_internal_format_to_fourcc(self.0.format?);
        if self.0.has_alpha {
            fmt
        } else {
            fmt.and_then(get_opaque)
        }
    }
}

/// Texture mapping of a GLES texture
#[derive(Debug)]
pub struct GlesMapping {
    pbo: ffi::types::GLuint,
    format: ffi::types::GLenum,
    layout: ffi::types::GLenum,
    has_alpha: bool,
    size: Size<i32, BufferCoord>,
    mapping: AtomicPtr<nix::libc::c_void>,
    destruction_callback_sender: Sender<CleanupResource>,
}

impl Texture for GlesMapping {
    fn width(&self) -> u32 {
        self.size.w as u32
    }
    fn height(&self) -> u32 {
        self.size.h as u32
    }
    fn size(&self) -> Size<i32, BufferCoord> {
        self.size
    }
    fn format(&self) -> Option<Fourcc> {
        let fmt = gl_read_format_to_fourcc(self.format, self.layout);
        if self.has_alpha {
            fmt
        } else {
            fmt.and_then(get_opaque)
        }
    }
}
impl TextureMapping for GlesMapping {
    fn flipped(&self) -> bool {
        true
    }
    fn format(&self) -> Fourcc {
        Texture::format(self).expect("Should never happen")
    }
}

impl Drop for GlesMapping {
    fn drop(&mut self) {
        let _ = self.destruction_callback_sender.send(CleanupResource::Mapping(
            self.pbo,
            self.mapping.load(Ordering::SeqCst),
        ));
    }
}

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
    destruction_callback_sender: Sender<CleanupResource>,
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
        // Gles2Buffer caches the shadow buffer and is renderer-local, so it works around the issue outlined below.
        // TODO: Ideally we would be able to share the texture between renderers with shared EGLContexts though.
        // But we definitly don't want to add user data to a dmabuf to facilitate this. Maybe use the EGLContexts userdata for storing the buffers?
        buf: GlesBuffer,
        dmabuf: Dmabuf,
    },
    Surface(Rc<EGLSurface>),
    Texture {
        texture: GlesTexture,
        fbo: ffi::types::GLuint,
        destruction_callback_sender: Sender<CleanupResource>,
    },
    Renderbuffer {
        buf: GlesRenderbuffer,
        fbo: ffi::types::GLuint,
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
            GlesTarget::Surface(surf) => {
                let format = surf.pixel_format();
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

/// A renderer utilizing OpenGL ES
pub struct GlesRenderer {
    buffers: Vec<GlesBuffer>,
    target: Option<GlesTarget>,
    pub(crate) extensions: Vec<String>,

    tex_program: GlesTexProgram,
    solid_program: GlesSolidProgram,

    dmabuf_cache: std::collections::HashMap<WeakDmabuf, GlesTexture>,
    egl: EGLContext,
    #[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
    egl_reader: Option<EGLBufferReader>,

    gl_version: version::GlVersion,
    vbos: [ffi::types::GLuint; 2],
    gl: ffi::Gles2,
    destruction_callback: Receiver<CleanupResource>,
    destruction_callback_sender: Sender<CleanupResource>,

    min_filter: TextureFilter,
    max_filter: TextureFilter,
    supports_instancing: bool,
    debug_flags: DebugFlags,
    _not_send: *mut (),
    span: tracing::Span,
    gl_debug_span: Option<*mut tracing::Span>,
}

struct RendererId(usize);
impl Drop for RendererId {
    fn drop(&mut self) {
        RENDERER_IDS.lock().unwrap().remove(&self.0);
    }
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
            .field("tex_program", &self.tex_program)
            .field("solid_program", &self.solid_program)
            .field("dmabuf_cache", &self.dmabuf_cache)
            .field("egl", &self.egl)
            .field("gl_version", &self.gl_version)
            // ffi::Gles does not implement Debug
            .field("vbos", &self.vbos)
            .field("min_filter", &self.min_filter)
            .field("max_filter", &self.max_filter)
            .field("supports_instancing", &self.supports_instancing)
            .finish()
    }
}

/// Error returned during rendering using GL ES
#[derive(thiserror::Error, Debug)]
pub enum GlesError {
    /// A shader could not be compiled
    #[error("Failed to compile Shader")]
    ShaderCompileError,
    /// A program could not be linked
    #[error("Failed to link Program")]
    ProgramLinkError,
    /// A framebuffer could not be bound
    #[error("Failed to bind Framebuffer")]
    FramebufferBindingError,
    /// Required GL functions could not be loaded
    #[error("Failed to load GL functions from EGL")]
    GLFunctionLoaderError,
    /// Required GL extension are not supported by the underlying implementation
    #[error("None of the following GL extensions is supported by the underlying GL implementation, at least one is required: {0:?}")]
    GLExtensionNotSupported(&'static [&'static str]),
    /// Required EGL extension are not supported by the underlying implementation
    #[error("None of the following EGL extensions is supported by the underlying implementation, at least one is required: {0:?}")]
    EGLExtensionNotSupported(&'static [&'static str]),
    /// Required GL version is not available by the underlying implementation
    #[error(
        "The OpenGL ES version of the underlying GL implementation is too low, at least required: {0:?}"
    )]
    GLVersionNotSupported(version::GlVersion),
    /// The underlying egl context could not be activated
    #[error("Failed to active egl context")]
    ContextActivationError(#[from] crate::backend::egl::MakeCurrentError),
    ///The given dmabuf could not be converted to an EGLImage for framebuffer use
    #[error("Failed to convert between dmabuf and EGLImage")]
    BindBufferEGLError(#[source] crate::backend::egl::Error),
    /// The given buffer has an unknown pixel format
    #[error("Unknown pixel format")]
    UnknownPixelFormat,
    /// The given buffer has an unsupported pixel format
    #[error("Unsupported pixel format: {0:?}")]
    UnsupportedPixelFormat(Fourcc),
    /// The given buffer has an unknown pixel layout
    #[error("Unsupported pixel layout")]
    UnsupportedPixelLayout,
    /// The given wl buffer has an unsupported pixel format
    #[error("Unsupported wl_shm format: {0:?}")]
    #[cfg(feature = "wayland_frontend")]
    UnsupportedWlPixelFormat(wl_shm::Format),
    /// The given buffer was not accessible
    #[error("Error accessing the buffer ({0:?})")]
    #[cfg(feature = "wayland_frontend")]
    BufferAccessError(crate::wayland::shm::BufferAccessError),
    /// The given egl buffer was not accessible
    #[error("Error accessing the buffer ({0:?})")]
    #[cfg(feature = "wayland_frontend")]
    EGLBufferAccessError(crate::backend::egl::BufferAccessError),
    /// This rendering operation was called without a previous `begin`-call
    #[error("Call begin before doing any rendering operations")]
    UnconstraintRenderingOperation,
    /// There was an error mapping the buffer
    #[error("Error mapping the buffer")]
    MappingError,
    /// The provided buffer's size did not match the requested one.
    #[error("Error reading buffer, size is too small for the given dimensions")]
    UnexpectedSize,
    /// The blitting operation was unsuccessful
    #[error("Error blitting between framebuffers")]
    BlitError,
    /// An error occured while creating the shader object.
    #[error("An error occured while creating the shader object.")]
    CreateShaderObject,
    /// Uniform was not declared when compiling shader
    #[error("Uniform {0:?} was not declared when compiling the provided shader")]
    UnknownUniform(String),
    /// The provided uniform has a different type then was provided when compiling the shader
    #[error("Uniform with different type (got {provided:?}, expected: {declared:?})")]
    UniformTypeMismatch {
        /// Uniform type that was provided during the call
        provided: UniformType,
        /// Uniform type that was declared when compiling
        declared: UniformType,
    },
}

impl From<GlesError> for SwapBuffersError {
    #[cfg(feature = "wayland_frontend")]
    fn from(err: GlesError) -> SwapBuffersError {
        match err {
            x @ GlesError::ShaderCompileError
            | x @ GlesError::ProgramLinkError
            | x @ GlesError::GLFunctionLoaderError
            | x @ GlesError::GLExtensionNotSupported(_)
            | x @ GlesError::EGLExtensionNotSupported(_)
            | x @ GlesError::GLVersionNotSupported(_)
            | x @ GlesError::UnconstraintRenderingOperation => SwapBuffersError::ContextLost(Box::new(x)),
            GlesError::ContextActivationError(err) => err.into(),
            x @ GlesError::FramebufferBindingError
            | x @ GlesError::BindBufferEGLError(_)
            | x @ GlesError::UnknownPixelFormat
            | x @ GlesError::UnsupportedPixelFormat(_)
            | x @ GlesError::UnsupportedWlPixelFormat(_)
            | x @ GlesError::UnsupportedPixelLayout
            | x @ GlesError::BufferAccessError(_)
            | x @ GlesError::MappingError
            | x @ GlesError::UnexpectedSize
            | x @ GlesError::BlitError
            | x @ GlesError::CreateShaderObject
            | x @ GlesError::UniformTypeMismatch { .. }
            | x @ GlesError::UnknownUniform(_)
            | x @ GlesError::EGLBufferAccessError(_) => SwapBuffersError::TemporaryFailure(Box::new(x)),
        }
    }
    #[cfg(not(feature = "wayland_frontend"))]
    fn from(err: GlesError) -> SwapBuffersError {
        match err {
            x @ GlesError::ShaderCompileError
            | x @ GlesError::ProgramLinkError
            | x @ GlesError::GLFunctionLoaderError
            | x @ GlesError::GLExtensionNotSupported(_)
            | x @ GlesError::EGLExtensionNotSupported(_)
            | x @ GlesError::GLVersionNotSupported(_)
            | x @ GlesError::UnconstraintRenderingOperation => SwapBuffersError::ContextLost(Box::new(x)),
            GlesError::ContextActivationError(err) => err.into(),
            x @ GlesError::FramebufferBindingError
            | x @ GlesError::MappingError
            | x @ GlesError::UnknownPixelFormat
            | x @ GlesError::UnsupportedPixelFormat(_)
            | x @ GlesError::UnsupportedPixelLayout
            | x @ GlesError::UnexpectedSize
            | x @ GlesError::BlitError
            | x @ GlesError::CreateShaderObject
            | x @ GlesError::UniformTypeMismatch { .. }
            | x @ GlesError::UnknownUniform(_)
            | x @ GlesError::BindBufferEGLError(_) => SwapBuffersError::TemporaryFailure(Box::new(x)),
        }
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

unsafe fn compile_shader(
    gl: &ffi::Gles2,
    variant: ffi::types::GLuint,
    src: &str,
) -> Result<ffi::types::GLuint, GlesError> {
    let shader = gl.CreateShader(variant);
    if shader == 0 {
        return Err(GlesError::CreateShaderObject);
    }

    gl.ShaderSource(
        shader,
        1,
        &src.as_ptr() as *const *const u8 as *const *const ffi::types::GLchar,
        &(src.len() as i32) as *const _,
    );
    gl.CompileShader(shader);

    let mut status = ffi::FALSE as i32;
    gl.GetShaderiv(shader, ffi::COMPILE_STATUS, &mut status as *mut _);
    if status == ffi::FALSE as i32 {
        let mut max_len = 0;
        gl.GetShaderiv(shader, ffi::INFO_LOG_LENGTH, &mut max_len as *mut _);

        let mut error = Vec::with_capacity(max_len as usize);
        let mut len = 0;
        gl.GetShaderInfoLog(
            shader,
            max_len as _,
            &mut len as *mut _,
            error.as_mut_ptr() as *mut _,
        );
        error.set_len(len as usize);

        error!(
            "[GL] {}",
            std::str::from_utf8(&error).unwrap_or("<Error Message no utf8>")
        );

        gl.DeleteShader(shader);
        return Err(GlesError::ShaderCompileError);
    }

    Ok(shader)
}

unsafe fn link_program(
    gl: &ffi::Gles2,
    vert_src: &str,
    frag_src: &str,
) -> Result<ffi::types::GLuint, GlesError> {
    let vert = compile_shader(gl, ffi::VERTEX_SHADER, vert_src)?;
    let frag = compile_shader(gl, ffi::FRAGMENT_SHADER, frag_src)?;
    let program = gl.CreateProgram();
    gl.AttachShader(program, vert);
    gl.AttachShader(program, frag);
    gl.LinkProgram(program);
    gl.DetachShader(program, vert);
    gl.DetachShader(program, frag);
    gl.DeleteShader(vert);
    gl.DeleteShader(frag);

    let mut status = ffi::FALSE as i32;
    gl.GetProgramiv(program, ffi::LINK_STATUS, &mut status as *mut _);
    if status == ffi::FALSE as i32 {
        let mut max_len = 0;
        gl.GetProgramiv(program, ffi::INFO_LOG_LENGTH, &mut max_len as *mut _);

        let mut error = Vec::with_capacity(max_len as usize);
        let mut len = 0;
        gl.GetProgramInfoLog(
            program,
            max_len as _,
            &mut len as *mut _,
            error.as_mut_ptr() as *mut _,
        );
        error.set_len(len as usize);

        error!(
            "[GL] {}",
            std::str::from_utf8(&error).unwrap_or("<Error Message no utf8>")
        );

        gl.DeleteProgram(program);
        return Err(GlesError::ProgramLinkError);
    }

    Ok(program)
}

unsafe fn texture_program(
    gl: &ffi::Gles2,
    frag: &str,
    additional_uniforms: &[UniformName<'_>],
    destruction_callback_sender: Sender<CleanupResource>,
) -> Result<GlesTexProgram, GlesError> {
    let create_variant = |defines: &[&str]| -> Result<GlesTexProgramVariant, GlesError> {
        let mut src = String::from(frag);
        for define in defines {
            src = format!("#define {}\n{}", define, frag);
        }
        let shader = format!("#version 100\n{}", src);
        let program = unsafe { link_program(gl, shaders::VERTEX_SHADER, &shader)? };

        let debug_shader = format!("#version 100\n#define {}\n{}", shaders::DEBUG_FLAGS, src);
        let debug_program = unsafe { link_program(gl, shaders::VERTEX_SHADER, debug_shader.as_ref())? };

        let vert = CStr::from_bytes_with_nul(b"vert\0").expect("NULL terminated");
        let vert_position = CStr::from_bytes_with_nul(b"vert_position\0").expect("NULL terminated");
        let tex = CStr::from_bytes_with_nul(b"tex\0").expect("NULL terminated");
        let matrix = CStr::from_bytes_with_nul(b"matrix\0").expect("NULL terminated");
        let tex_matrix = CStr::from_bytes_with_nul(b"tex_matrix\0").expect("NULL terminated");
        let alpha = CStr::from_bytes_with_nul(b"alpha\0").expect("NULL terminated");
        let tint = CStr::from_bytes_with_nul(b"tint\0").expect("NULL terminated");

        Ok(GlesTexProgramVariant {
            normal: GlesTexProgramInternal {
                program,
                uniform_tex: gl.GetUniformLocation(program, tex.as_ptr() as *const ffi::types::GLchar),
                uniform_matrix: gl.GetUniformLocation(program, matrix.as_ptr() as *const ffi::types::GLchar),
                uniform_tex_matrix: gl
                    .GetUniformLocation(program, tex_matrix.as_ptr() as *const ffi::types::GLchar),
                uniform_alpha: gl.GetUniformLocation(program, alpha.as_ptr() as *const ffi::types::GLchar),
                attrib_vert: gl.GetAttribLocation(program, vert.as_ptr() as *const ffi::types::GLchar),
                attrib_vert_position: gl
                    .GetAttribLocation(program, vert_position.as_ptr() as *const ffi::types::GLchar),
                additional_uniforms: additional_uniforms
                    .iter()
                    .map(|uniform| {
                        let name = CString::new(uniform.name.as_bytes()).expect("Interior null in name");
                        let location =
                            gl.GetUniformLocation(program, name.as_ptr() as *const ffi::types::GLchar);
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
            debug: GlesTexProgramInternal {
                program: debug_program,
                uniform_tex: gl.GetUniformLocation(debug_program, tex.as_ptr() as *const ffi::types::GLchar),
                uniform_matrix: gl
                    .GetUniformLocation(debug_program, matrix.as_ptr() as *const ffi::types::GLchar),
                uniform_tex_matrix: gl
                    .GetUniformLocation(debug_program, tex_matrix.as_ptr() as *const ffi::types::GLchar),
                uniform_alpha: gl
                    .GetUniformLocation(debug_program, alpha.as_ptr() as *const ffi::types::GLchar),
                attrib_vert: gl.GetAttribLocation(debug_program, vert.as_ptr() as *const ffi::types::GLchar),
                attrib_vert_position: gl
                    .GetAttribLocation(debug_program, vert_position.as_ptr() as *const ffi::types::GLchar),
                additional_uniforms: additional_uniforms
                    .iter()
                    .map(|uniform| {
                        let name = CString::new(uniform.name.as_bytes()).expect("Interior null in name");
                        let location =
                            gl.GetUniformLocation(debug_program, name.as_ptr() as *const ffi::types::GLchar);
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
            // debug flags
            uniform_tint: gl.GetUniformLocation(debug_program, tint.as_ptr() as *const ffi::types::GLchar),
        })
    };

    Ok(GlesTexProgram(Rc::new(GlesTexProgramInner {
        variants: [
            create_variant(&[])?,
            create_variant(&[shaders::NO_ALPHA])?,
            create_variant(&[shaders::EXTERNAL])?,
        ],
        destruction_callback_sender,
    })))
}

unsafe fn solid_program(gl: &ffi::Gles2) -> Result<GlesSolidProgram, GlesError> {
    let program = link_program(gl, shaders::VERTEX_SHADER_SOLID, shaders::FRAGMENT_SHADER_SOLID)?;

    let matrix = CStr::from_bytes_with_nul(b"matrix\0").expect("NULL terminated");
    let color = CStr::from_bytes_with_nul(b"color\0").expect("NULL terminated");
    let vert = CStr::from_bytes_with_nul(b"vert\0").expect("NULL terminated");
    let position = CStr::from_bytes_with_nul(b"position\0").expect("NULL terminated");

    Ok(GlesSolidProgram {
        program,
        uniform_matrix: gl.GetUniformLocation(program, matrix.as_ptr() as *const ffi::types::GLchar),
        uniform_color: gl.GetUniformLocation(program, color.as_ptr() as *const ffi::types::GLchar),
        attrib_vert: gl.GetAttribLocation(program, vert.as_ptr() as *const ffi::types::GLchar),
        attrib_position: gl.GetAttribLocation(program, position.as_ptr() as *const ffi::types::GLchar),
    })
}

impl GlesRenderer {
    /// Creates a new OpenGL ES renderer from a given [`EGLContext`](crate::backend::egl::EGLContext).
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
    pub unsafe fn new(context: EGLContext) -> Result<GlesRenderer, GlesError> {
        let span = info_span!(parent: &context.span, "renderer_gles2");
        let _guard = span.enter();

        context.make_current()?;

        let (gl, gl_version, exts, supports_instancing, gl_debug_span) = {
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
            // Check if GPU supports instanced rendering.
            let supports_instancing = gl_version >= version::GLES_3_0
                || (exts.iter().any(|ext| ext == "GL_EXT_instanced_arrays")
                    && exts.iter().any(|ext| ext == "GL_EXT_draw_instanced"));

            let gl_debug_span = if exts.iter().any(|ext| ext == "GL_KHR_debug") {
                gl.Enable(ffi::DEBUG_OUTPUT);
                gl.Enable(ffi::DEBUG_OUTPUT_SYNCHRONOUS);
                let span = Box::into_raw(Box::new(span.clone()));
                gl.DebugMessageCallback(Some(gl_debug_log), span as *mut _);
                Some(span)
            } else {
                None
            };

            (gl, gl_version, exts, supports_instancing, gl_debug_span)
        };

        let (tx, rx) = channel();
        let tex_program = texture_program(&gl, shaders::FRAGMENT_SHADER, &[], tx.clone())?;
        let solid_program = solid_program(&gl)?;

        // Initialize vertices based on drawing methodology.
        let vertices: &[ffi::types::GLfloat] = if supports_instancing {
            &INSTANCED_VERTS
        } else {
            &TRIANGLE_VERTS
        };

        let mut vbos = [0; 2];
        gl.GenBuffers(vbos.len() as i32, vbos.as_mut_ptr());
        gl.BindBuffer(ffi::ARRAY_BUFFER, vbos[0]);
        gl.BufferData(
            ffi::ARRAY_BUFFER,
            (std::mem::size_of::<ffi::types::GLfloat>() * vertices.len()) as isize,
            vertices.as_ptr() as *const _,
            ffi::STATIC_DRAW,
        );
        gl.BindBuffer(ffi::ARRAY_BUFFER, 0);

        context
            .user_data()
            .insert_if_missing(|| RendererId(next_renderer_id()));

        drop(_guard);
        let renderer = GlesRenderer {
            gl,
            egl: context,
            #[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
            egl_reader: None,
            extensions: exts,
            gl_version,
            tex_program,
            solid_program,
            target: None,
            buffers: Vec::new(),
            dmabuf_cache: std::collections::HashMap::new(),
            destruction_callback: rx,
            destruction_callback_sender: tx,
            vbos,
            min_filter: TextureFilter::Linear,
            max_filter: TextureFilter::Linear,
            supports_instancing,
            debug_flags: DebugFlags::empty(),
            _not_send: std::ptr::null_mut(),
            span,
            gl_debug_span,
        };
        renderer.egl.unbind()?;
        Ok(renderer)
    }

    pub(crate) fn make_current(&mut self) -> Result<(), MakeCurrentError> {
        unsafe {
            if let Some(&GlesTarget::Surface(ref surface)) = self.target.as_ref() {
                self.egl.make_current_with_surface(surface)?;
            } else {
                self.egl.make_current()?;
                match self.target.as_ref() {
                    Some(&GlesTarget::Image { ref buf, .. }) => {
                        self.gl.BindFramebuffer(ffi::FRAMEBUFFER, buf.fbo)
                    }
                    Some(&GlesTarget::Texture { ref fbo, .. }) => {
                        self.gl.BindFramebuffer(ffi::FRAMEBUFFER, *fbo)
                    }
                    Some(&GlesTarget::Renderbuffer { ref fbo, .. }) => {
                        self.gl.BindFramebuffer(ffi::FRAMEBUFFER, *fbo)
                    }
                    _ => {}
                }
            }
        }
        // delayed destruction until the next frame rendering.
        self.cleanup();
        Ok(())
    }

    fn cleanup(&mut self) {
        #[cfg(feature = "wayland_frontend")]
        self.dmabuf_cache.retain(|entry, _tex| entry.upgrade().is_some());
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
            }
        }
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportMemWl for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
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

            let offset = data.offset as i32;
            let width = data.width as i32;
            let height = data.height as i32;
            let stride = data.stride as i32;
            let fourcc =
                shm_format_to_fourcc(data.format).ok_or(GlesError::UnsupportedWlPixelFormat(data.format))?;

            if !SUPPORTED_MEM_FORMATS_2.contains(&fourcc) {
                return Err(Gles2Error::UnsupportedWlPixelFormat(data.format));
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

const SUPPORTED_MEM_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr8888,
    Fourcc::Xbgr8888,
    Fourcc::Argb8888,
    Fourcc::Xrgb8888,
];

impl ImportMem for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
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

        if !SUPPORTED_MEM_FORMATS.contains(&format) {
            return Err(Gles2Error::UnsupportedPixelFormat(format));
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
                    layout as u32,
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
                read_format as u32,
                type_ as u32,
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
        Box::new(SUPPORTED_MEM_FORMATS.iter().copied())
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
            let has_alpha = has_alpha(buffer.format().code);
            let texture = GlesTexture(Rc::new(GlesTextureInternal {
                texture: tex,
                format: Some(ffi::RGBA8),
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
    fn existing_dmabuf_texture(&self, buffer: &Dmabuf) -> Result<Option<GlesTexture>, GlesError> {
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

    fn import_egl_image(
        &self,
        image: EGLImage,
        is_external: bool,
        tex: Option<u32>,
    ) -> Result<u32, GlesError> {
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
    fn copy_framebuffer(
        &mut self,
        region: Rectangle<i32, BufferCoord>,
    ) -> Result<Self::TextureMapping, Self::Error> {
        self.make_current()?;

        let (internal, has_alpha) = self
            .target
            .as_ref()
            .ok_or(GlesError::UnknownPixelFormat)?
            .format()
            .ok_or(GlesError::UnknownPixelFormat)?;
        let (format, layout) = gl_read_for_internal(internal).ok_or(GlesError::UnknownPixelFormat)?;

        let mut pbo = 0;
        unsafe {
            self.gl.GenBuffers(1, &mut pbo);
            self.gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, pbo);
            let size = (region.size.w
                * region.size.h
                * (gl_bpp(format, layout).ok_or(GlesError::UnsupportedPixelLayout)? / 8) as i32)
                as isize;
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
        }
        Ok(GlesMapping {
            pbo,
            format,
            layout,
            has_alpha,
            size: region.size,
            mapping: AtomicPtr::new(ptr::null_mut()),
            destruction_callback_sender: self.destruction_callback_sender.clone(),
        })
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, BufferCoord>,
    ) -> Result<Self::TextureMapping, Self::Error> {
        let mut pbo = 0;
        let old_target = self.target.take();
        self.bind(texture.clone())?;

        let (format, layout) = gl_read_for_internal(texture.0.format.ok_or(GlesError::UnknownPixelFormat)?)
            .ok_or(GlesError::UnknownPixelFormat)?;

        unsafe {
            self.gl.GenBuffers(1, &mut pbo);
            self.gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, pbo);
            self.gl.BufferData(
                ffi::PIXEL_PACK_BUFFER,
                (region.size.w
                    * region.size.h
                    * (gl_bpp(format, layout).expect("We check the format before") / 8) as i32)
                    as isize,
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
        }

        // restore old framebuffer
        self.unbind()?;
        self.target = old_target;
        self.make_current()?;

        Ok(GlesMapping {
            pbo,
            format,
            layout,
            has_alpha: texture.0.has_alpha,
            size: region.size,
            mapping: AtomicPtr::new(ptr::null_mut()),
            destruction_callback_sender: self.destruction_callback_sender.clone(),
        })
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
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

impl ExportDma for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    fn export_texture(&mut self, texture: &GlesTexture) -> Result<Dmabuf, GlesError> {
        self.make_current()?;

        if !self
            .egl
            .display()
            .extensions()
            .iter()
            .any(|s| s == "EGL_KHR_gl_texture_2D_image")
        {
            return Err(GlesError::EGLExtensionNotSupported(&[
                "EGL_KHR_gl_texture_2D_image",
            ]));
        }

        let image = if let Some(egl_images) = texture.0.egl_images.as_ref() {
            egl_images[0]
        } else {
            unsafe {
                let attributes: [ffi_egl::types::EGLAttrib; 3] = [
                    ffi_egl::IMAGE_PRESERVED as ffi_egl::types::EGLAttrib,
                    ffi_egl::TRUE as ffi_egl::types::EGLAttrib,
                    ffi_egl::NONE as ffi_egl::types::EGLAttrib,
                ];
                let img = ffi_egl::CreateImage(
                    **self.egl.display().get_display_handle(),
                    self.egl.get_context_handle(),
                    ffi_egl::GL_TEXTURE_2D,
                    texture.0.texture as ffi_egl::types::EGLClientBuffer,
                    attributes.as_ptr() as *const _,
                );
                if img == ffi_egl::NO_IMAGE_KHR {
                    return Err(GlesError::BindBufferEGLError(
                        crate::backend::egl::Error::EGLImageCreationFailed,
                    ));
                }
                img
            }
        };

        let res = self
            .egl
            .display()
            .create_dmabuf_from_image(image, texture.size(), true)
            .map_err(GlesError::BindBufferEGLError);
        unsafe { ffi_egl::DestroyImageKHR(**self.egl.display().get_display_handle(), image) };
        res
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    fn export_framebuffer(&mut self, size: Size<i32, BufferCoord>) -> Result<Dmabuf, GlesError> {
        self.make_current()?;

        if !self
            .egl
            .display()
            .extensions()
            .iter()
            .any(|s| s == "EGL_KHR_gl_renderbuffer_image")
        {
            return Err(GlesError::EGLExtensionNotSupported(&[
                "EGL_KHR_gl_renderbuffer_image",
            ]));
        }

        let rbo = match self.target.as_ref() {
            Some(&GlesTarget::Image { ref dmabuf, .. }) => return Ok(dmabuf.clone()),
            Some(&GlesTarget::Texture { ref texture, .. }) => {
                // work around immutable borrow of self..
                let texture = texture.clone();
                return self.export_texture(&texture);
            }
            Some(&GlesTarget::Renderbuffer { ref buf, .. }) => buf.0.rbo,
            _ => unsafe {
                let mut rbo = 0;
                self.gl.GenRenderbuffers(1, &mut rbo);
                rbo
            },
        };

        let image = unsafe {
            let img = ffi_egl::CreateImage(
                **self.egl.display().get_display_handle(),
                self.egl.get_context_handle(),
                ffi_egl::GL_RENDERBUFFER,
                rbo as ffi_egl::types::EGLClientBuffer,
                ptr::null(),
            );
            if img == ffi_egl::NO_IMAGE_KHR {
                return Err(GlesError::BindBufferEGLError(
                    crate::backend::egl::Error::EGLImageCreationFailed,
                ));
            }
            img
        };

        if !matches!(self.target.as_ref(), Some(&GlesTarget::Renderbuffer { .. })) {
            // At this point the user tries to copy from an EGLSurface or another
            // default framebuffer, we need glBlitFramebuffer to do this, which
            // only exists for GL ES 3.0 and higher.
            if self.gl_version < version::GLES_3_0 {
                return Err(GlesError::GLVersionNotSupported(version::GLES_3_0));
            }

            unsafe {
                let mut fbo = 0;
                self.gl.GenFramebuffers(1, &mut fbo as *mut _);
                self.gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);
                self.gl.FramebufferRenderbuffer(
                    ffi::DRAW_FRAMEBUFFER,
                    ffi::COLOR_ATTACHMENT0,
                    ffi::RENDERBUFFER,
                    rbo,
                );
                let status = self.gl.CheckFramebufferStatus(ffi::DRAW_FRAMEBUFFER);

                if status != ffi::FRAMEBUFFER_COMPLETE {
                    //TODO wrap image and drop here
                    return Err(GlesError::FramebufferBindingError);
                }

                self.gl.BlitFramebuffer(
                    0,
                    0,
                    size.w,
                    size.h,
                    0,
                    0,
                    size.w,
                    size.h,
                    ffi::COLOR_BUFFER_BIT,
                    ffi::NEAREST,
                );
                self.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
            }
            // reset framebuffer
            self.make_current()?;
        };

        let res = self
            .egl
            .display()
            .create_dmabuf_from_image(image, size, true)
            .map_err(GlesError::BindBufferEGLError);
        unsafe {
            ffi_egl::DestroyImageKHR(**self.egl.display().get_display_handle(), image);
        }
        res
    }
}

impl Bind<Rc<EGLSurface>> for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    fn bind(&mut self, surface: Rc<EGLSurface>) -> Result<(), GlesError> {
        self.unbind()?;
        self.target = Some(GlesTarget::Surface(surface));
        self.make_current()?;
        Ok(())
    }
}

impl Bind<Dmabuf> for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    fn bind(&mut self, dmabuf: Dmabuf) -> Result<(), GlesError> {
        self.unbind()?;
        self.make_current()?;

        let (buf, dmabuf) = self
            .buffers
            .iter()
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

                    let buf = GlesBuffer {
                        dmabuf: dmabuf.weak(),
                        image,
                        rbo,
                        fbo,
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
    fn bind(&mut self, texture: GlesTexture) -> Result<(), GlesError> {
        self.unbind()?;
        self.make_current()?;

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

        self.target = Some(GlesTarget::Texture {
            texture,
            destruction_callback_sender: self.destruction_callback_sender.clone(),
            fbo,
        });
        self.make_current()?;

        Ok(())
    }
}

impl Offscreen<GlesTexture> for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
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

        Ok(unsafe { GlesTexture::from_raw(self, Some(internal), has_alpha, tex, size) })
    }
}

impl Bind<GlesRenderbuffer> for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    fn bind(&mut self, renderbuffer: GlesRenderbuffer) -> Result<(), GlesError> {
        self.unbind()?;
        self.make_current()?;

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

        self.target = Some(GlesTarget::Renderbuffer {
            buf: renderbuffer,
            fbo,
        });
        self.make_current()?;

        Ok(())
    }
}

impl Offscreen<GlesRenderbuffer> for GlesRenderer {
    #[instrument(level = "trace", parent = &self.span, skip(self))]
    fn create_buffer(
        &mut self,
        format: Fourcc,
        size: Size<i32, BufferCoord>,
    ) -> Result<GlesRenderbuffer, GlesError> {
        self.make_current()?;

        let has_alpha = has_alpha(format);
        let (internal, _, _) = fourcc_to_gl_formats(if has_alpha {
            format
        } else {
            get_transparent(format).ok_or(GlesError::UnsupportedPixelFormat(format))?
        })
        .ok_or(GlesError::UnsupportedPixelFormat(format))?;

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
            (&GlesTarget::Surface(ref src), &GlesTarget::Surface(ref dst)) => unsafe {
                self.egl.make_current_with_draw_and_read_surface(dst, src)?;
            },
            (&GlesTarget::Surface(ref src), _) => unsafe {
                self.egl.make_current_with_surface(src)?;
            },
            (_, &GlesTarget::Surface(ref dst)) => unsafe {
                self.egl.make_current_with_surface(dst)?;
            },
            (_, _) => unsafe {
                self.egl.make_current()?;
            },
        }

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
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error> {
        unsafe {
            self.egl.make_current()?;
        }
        unsafe { self.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0) };
        self.target = None;
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
        self.make_current()?;
        Ok(func(&self.gl))
    }

    /// Compile a custom pixel shader for rendering with [`GlesRenderer::render_pixel_shader_to`].
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
    /// and can then be set in functions utilizing `GlesPixelProgram` (like [`Gles2Renderer::render_pixel_shader_to`]).
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

    /// Compile a custom texture shader for rendering with [`GlesRenderer::render_texture`] or [`Gles2Renderer::render_texture_from_to`].
    ///
    /// They need to handle the following #define variants:
    /// - `EXTERNAL` uses samplerExternalOES instead of sampler2D, requires the GL_OES_EGL_image_external extension
    /// - `XBGR` needs to ignore the alpha channel of the texture and replace it with 1.0
    /// - `DEBUG_FLAGS` see below
    ///
    /// They receive the following variables:
    /// - *varying* v_coords `vec2` - contains the position from the vertex shader
    /// - *uniform* tex `sample2d` - texture sampler
    /// - *uniform* alpha `float` - for the alpha value passed by the renderer
    /// - *uniform* tint `float` - for the tint passed by the renderer (either 0.0 or 1.0) - only if `DEBUG_FLAGS` was defined
    ///
    /// Additional uniform values can be defined by passing `UniformName`s to the `additional_uniforms` argument
    /// and can then be set in functions utilizing `GlesTexProgram` (like [`Gles2Renderer::render_texture`] or [`Gles2Renderer::render_texture_from_to`]).
    ///
    /// The shader must **not** contain a `#version` directive. It will be interpreted as version 100.
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

    fn render(
        &mut self,
        mut output_size: Size<i32, Physical>,
        transform: Transform,
    ) -> Result<GlesFrame<'_>, Self::Error> {
        self.make_current()?;

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
            // output transformation passed in by the user
            current_projection,
            transform,
            size: output_size,
            tex_program_override: None,
            finished: AtomicBool::new(false),
            span,
        })
    }

    fn set_debug_flags(&mut self, flags: DebugFlags) {
        self.debug_flags = flags;
    }

    fn debug_flags(&self) -> DebugFlags {
        self.debug_flags
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

impl<'frame> Frame for GlesFrame<'frame> {
    type TextureId = GlesTexture;
    type Error = GlesError;

    fn id(&self) -> usize {
        self.renderer.id()
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    fn clear(&mut self, color: [f32; 4], at: &[Rectangle<i32, Physical>]) -> Result<(), GlesError> {
        if at.is_empty() {
            return Ok(());
        }

        unsafe {
            self.renderer.gl.Disable(ffi::BLEND);
        }

        let res = self.draw_solid(Rectangle::from_loc_and_size((0, 0), self.size), at, color);

        unsafe {
            self.renderer.gl.Enable(ffi::BLEND);
            self.renderer.gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
        }
        res
    }

    #[instrument(level = "trace", skip(self), parent = &self.span)]
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

    fn finish(mut self) -> Result<(), Self::Error> {
        self.finish_internal()
    }
}

impl<'frame> GlesFrame<'frame> {
    fn finish_internal(&mut self) -> Result<(), GlesError> {
        let _guard = self.span.enter();

        if self.finished.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        unsafe {
            self.renderer.gl.Flush();
            // We need to wait for the previously submitted GL commands to complete
            // or otherwise the buffer could be submitted to the drm surface while
            // still writing to the buffer which results in flickering on the screen.
            // The proper solution would be to create a fence just before calling
            // glFlush that the backend can use to wait for the commands to be finished.
            // In case of a drm atomic backend the fence could be supplied by using the
            // IN_FENCE_FD property.
            // See https://01.org/linuxgraphics/gfx-docs/drm/gpu/drm-kms.html#explicit-fencing-properties for
            // the topic on submitting a IN_FENCE_FD and the mesa kmskube example
            // https://gitlab.freedesktop.org/mesa/kmscube/-/blob/9f63f359fab1b5d8e862508e4e51c9dfe339ccb0/drm-atomic.c
            // especially here
            // https://gitlab.freedesktop.org/mesa/kmscube/-/blob/9f63f359fab1b5d8e862508e4e51c9dfe339ccb0/drm-atomic.c#L147
            // and here
            // https://gitlab.freedesktop.org/mesa/kmscube/-/blob/9f63f359fab1b5d8e862508e4e51c9dfe339ccb0/drm-atomic.c#L235
            self.renderer.gl.Finish();
            self.renderer.gl.Disable(ffi::BLEND);
        }
        Ok(())
    }

    /// Overrides the default texture shader used, if none is specified.
    ///
    /// This affects calls to [`Frame::render_texture_at`] or [`Frame::render_texture_from_to`] as well as
    /// calls to [`GlesFrame::render_texture_to`] or [`Gles2Frame::render_texture`], if the passed in `program` is `None`.
    ///
    /// Override is active only for the lifetime of this `GlesFrame` and can be reset via [`Gles2Frame::clear_tex_program_override`].
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
            let vertices = if self.renderer.supports_instancing {
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
            if self.renderer.supports_instancing {
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
    #[instrument(skip(self), parent = &self.span)]
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
            transform.matrix()
        } else {
            transform.invert().matrix()
        };

        if src_size.w == 0. || src_size.h == 0. || tex_size.w == 0. || tex_size.h == 0. {
            warn!("Texture/Src is zero sized");
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
        unsafe {
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
            let vertices = if self.renderer.supports_instancing {
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

            let damage_len = (damage.len() / 4) as i32;
            if self.renderer.supports_instancing {
                gl.VertexAttribDivisor(program.attrib_vert as u32, 0);
                gl.VertexAttribDivisor(program.attrib_vert_position as u32, 1);

                gl.DrawArraysInstanced(ffi::TRIANGLE_STRIP, 0, 4, damage_len);
            } else {
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
            }

            gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
            gl.BindTexture(target, 0);
            gl.DisableVertexAttribArray(program.attrib_vert as u32);
            gl.DisableVertexAttribArray(program.attrib_vert_position as u32);
        }

        Ok(())
    }

    /// Render a pixel shader into the current target at a given `dest`-region.
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
            let vertices = if self.renderer.supports_instancing {
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
            if self.renderer.supports_instancing {
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
        if let Err(err) = self.finish_internal() {
            warn!("Ignored error finishing GlesFrame on drop: {}", err);
        }
    }
}
