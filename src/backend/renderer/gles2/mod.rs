//! Implementation of the rendering traits using OpenGL ES 2

use std::cell::RefCell;
use std::convert::TryFrom;
use std::ffi::CStr;
use std::fmt;
use std::ptr;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc::{channel, Receiver, Sender},
};
use std::{collections::HashSet, os::raw::c_char};

use cgmath::{prelude::*, Matrix3};

mod shaders;
mod version;

use super::{Bind, Renderer, Texture, Transform, Unbind};
use crate::backend::allocator::{
    dmabuf::{Dmabuf, WeakDmabuf},
    Format,
};
use crate::backend::egl::{
    display::EGLBufferReader, ffi::egl::types::EGLImage, EGLBuffer, EGLContext, EGLSurface,
    Format as EGLFormat, MakeCurrentError,
};
use crate::backend::SwapBuffersError;

#[cfg(feature = "wayland_frontend")]
use crate::wayland::compositor::SurfaceAttributes;
#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_buffer, wl_shm};

#[allow(clippy::all, missing_docs)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/gl_bindings.rs"));
}

static RENDERER_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
struct Gles2Program {
    program: ffi::types::GLuint,
    uniform_tex: ffi::types::GLint,
    uniform_matrix: ffi::types::GLint,
    uniform_invert_y: ffi::types::GLint,
    uniform_alpha: ffi::types::GLint,
    attrib_position: ffi::types::GLint,
    attrib_tex_coords: ffi::types::GLint,
}

/// A handle to a GLES2 texture
#[derive(Debug, Clone)]
pub struct Gles2Texture(Rc<Gles2TextureInternal>);

#[derive(Debug)]
struct Gles2TextureInternal {
    texture: ffi::types::GLuint,
    texture_kind: usize,
    is_external: bool,
    y_inverted: bool,
    width: u32,
    height: u32,
    destruction_callback_sender: Sender<ffi::types::GLuint>,
}

impl Drop for Gles2TextureInternal {
    fn drop(&mut self) {
        let _ = self.destruction_callback_sender.send(self.texture);
    }
}

impl Texture for Gles2Texture {
    fn width(&self) -> u32 {
        self.0.width
    }
    fn height(&self) -> u32 {
        self.0.height
    }
}

#[derive(Debug, Clone)]
struct WeakGles2Buffer {
    dmabuf: WeakDmabuf,
    image: EGLImage,
    rbo: ffi::types::GLuint,
    fbo: ffi::types::GLuint,
}

#[derive(Debug)]
struct Gles2Buffer {
    internal: WeakGles2Buffer,
    _dmabuf: Dmabuf,
}

/// A renderer utilizing OpenGL ES 2
pub struct Gles2Renderer {
    id: usize,
    buffers: Vec<WeakGles2Buffer>,
    target_buffer: Option<Gles2Buffer>,
    target_surface: Option<Rc<EGLSurface>>,
    current_projection: Option<Matrix3<f32>>,
    extensions: Vec<String>,
    programs: [Gles2Program; shaders::FRAGMENT_COUNT],
    gl: ffi::Gles2,
    egl: EGLContext,
    destruction_callback: Receiver<ffi::types::GLuint>,
    destruction_callback_sender: Sender<ffi::types::GLuint>,
    logger: Option<*mut ::slog::Logger>,
    _not_send: *mut (),
}

impl fmt::Debug for Gles2Renderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gles2Renderer")
            .field("id", &self.id)
            .field("buffers", &self.buffers)
            .field("target_buffer", &self.target_buffer)
            .field("target_surface", &self.target_surface)
            .field("current_projection", &self.current_projection)
            .field("extensions", &self.extensions)
            .field("programs", &self.programs)
            // ffi::Gles2 does not implement Debug
            .field("egl", &self.egl)
            .field("logger", &self.logger)
            .finish()
    }
}

/// Error returned during rendering using GL ES
#[derive(thiserror::Error, Debug)]
pub enum Gles2Error {
    /// A shader could not be compiled
    #[error("Failed to compile Shader: {0}")]
    ShaderCompileError(&'static str),
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
    /// The underlying egl context could not be activated
    #[error("Failed to active egl context")]
    ContextActivationError(#[from] crate::backend::egl::MakeCurrentError),
    ///The given dmabuf could not be converted to an EGLImage for framebuffer use
    #[error("Failed to convert dmabuf to EGLImage")]
    BindBufferEGLError(#[source] crate::backend::egl::Error),
    /// The given buffer has an unsupported pixel format
    #[error("Unsupported pixel format: {0:?}")]
    #[cfg(feature = "wayland_frontend")]
    UnsupportedPixelFormat(wl_shm::Format),
    /// The given buffer was not accessible
    #[error("Error accessing the buffer ({0:?})")]
    #[cfg(feature = "wayland_frontend")]
    BufferAccessError(crate::wayland::shm::BufferAccessError),
    /// The given egl buffer was not accessible
    #[error("Error accessing the buffer ({0:?})")]
    #[cfg(feature = "wayland_frontend")]
    EGLBufferAccessError(crate::backend::egl::BufferAccessError),
    /// The buffer backend is unknown or unsupported
    #[error("Error accessing the buffer")]
    #[cfg(feature = "wayland_frontend")]
    UnknownBufferType,
    /// This rendering operation was called without a previous `begin`-call
    #[error("Call begin before doing any rendering operations")]
    UnconstraintRenderingOperation,
}

impl From<Gles2Error> for SwapBuffersError {
    fn from(err: Gles2Error) -> SwapBuffersError {
        match err {
            x @ Gles2Error::ShaderCompileError(_)
            | x @ Gles2Error::ProgramLinkError
            | x @ Gles2Error::GLFunctionLoaderError
            | x @ Gles2Error::GLExtensionNotSupported(_)
            | x @ Gles2Error::UnconstraintRenderingOperation => SwapBuffersError::ContextLost(Box::new(x)),
            Gles2Error::ContextActivationError(err) => err.into(),
            x @ Gles2Error::FramebufferBindingError
            | x @ Gles2Error::BindBufferEGLError(_)
            | x @ Gles2Error::UnsupportedPixelFormat(_)
            | x @ Gles2Error::UnknownBufferType
            | x @ Gles2Error::BufferAccessError(_)
            | x @ Gles2Error::EGLBufferAccessError(_) => SwapBuffersError::TemporaryFailure(Box::new(x)),
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
        let msg = CStr::from_ptr(message);
        let log = Box::from_raw(user_param as *mut ::slog::Logger);
        let message_utf8 = msg.to_string_lossy();
        match gltype {
            ffi::DEBUG_TYPE_ERROR | ffi::DEBUG_TYPE_UNDEFINED_BEHAVIOR => {
                error!(log, "[GL] {}", message_utf8)
            }
            ffi::DEBUG_TYPE_DEPRECATED_BEHAVIOR => warn!(log, "[GL] {}", message_utf8),
            _ => debug!(log, "[GL] {}", message_utf8),
        };
        std::mem::forget(log);
    });
}

unsafe fn compile_shader(
    gl: &ffi::Gles2,
    variant: ffi::types::GLuint,
    src: &'static str,
) -> Result<ffi::types::GLuint, Gles2Error> {
    let shader = gl.CreateShader(variant);
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
        gl.DeleteShader(shader);
        return Err(Gles2Error::ShaderCompileError(src));
    }

    Ok(shader)
}

unsafe fn link_program(
    gl: &ffi::Gles2,
    vert_src: &'static str,
    frag_src: &'static str,
) -> Result<ffi::types::GLuint, Gles2Error> {
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
        gl.DeleteProgram(program);
        return Err(Gles2Error::ProgramLinkError);
    }

    Ok(program)
}

unsafe fn texture_program(gl: &ffi::Gles2, frag: &'static str) -> Result<Gles2Program, Gles2Error> {
    let program = link_program(&gl, shaders::VERTEX_SHADER, frag)?;

    let position = CStr::from_bytes_with_nul(b"position\0").expect("NULL terminated");
    let tex_coords = CStr::from_bytes_with_nul(b"tex_coords\0").expect("NULL terminated");
    let tex = CStr::from_bytes_with_nul(b"tex\0").expect("NULL terminated");
    let matrix = CStr::from_bytes_with_nul(b"matrix\0").expect("NULL terminated");
    let invert_y = CStr::from_bytes_with_nul(b"invert_y\0").expect("NULL terminated");
    let alpha = CStr::from_bytes_with_nul(b"alpha\0").expect("NULL terminated");

    Ok(Gles2Program {
        program,
        uniform_tex: gl.GetUniformLocation(program, tex.as_ptr() as *const ffi::types::GLchar),
        uniform_matrix: gl.GetUniformLocation(program, matrix.as_ptr() as *const ffi::types::GLchar),
        uniform_invert_y: gl.GetUniformLocation(program, invert_y.as_ptr() as *const ffi::types::GLchar),
        uniform_alpha: gl.GetUniformLocation(program, alpha.as_ptr() as *const ffi::types::GLchar),
        attrib_position: gl.GetAttribLocation(program, position.as_ptr() as *const ffi::types::GLchar),
        attrib_tex_coords: gl.GetAttribLocation(program, tex_coords.as_ptr() as *const ffi::types::GLchar),
    })
}

impl Gles2Renderer {
    /// Creates a new OpenGL ES 2 renderer from a given [`EGLContext`](backend::egl::EGLBuffer).
    ///
    /// # Safety
    ///
    /// This operation will cause undefined behavior if the given EGLContext is active in another thread.
    ///
    /// # Implementation details
    ///
    /// - Texture handles created by the resulting renderer are valid for every rendered created with an
    /// `EGLContext` shared with the given one (see `EGLContext::new_shared`) and can be used and destroyed on
    /// any of these renderers.
    /// - This renderer has no default framebuffer, use `Bind::bind` before rendering.
    /// - Shm buffers can be released after a successful import, without the texture handle becoming invalid.
    pub unsafe fn new<L>(context: EGLContext, logger: L) -> Result<Gles2Renderer, Gles2Error>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "renderer_gles2"));

        context.make_current()?;

        let (gl, exts, logger) = {
            let gl = ffi::Gles2::load_with(|s| crate::backend::egl::get_proc_address(s) as *const _);
            let ext_ptr = gl.GetString(ffi::EXTENSIONS) as *const c_char;
            if ext_ptr.is_null() {
                return Err(Gles2Error::GLFunctionLoaderError);
            }

            let exts = {
                let p = CStr::from_ptr(ext_ptr);
                let list = String::from_utf8(p.to_bytes().to_vec()).unwrap_or_else(|_| String::new());
                list.split(' ').map(|e| e.to_string()).collect::<Vec<_>>()
            };

            info!(log, "Initializing OpenGL ES Renderer");
            info!(
                log,
                "GL Version: {:?}",
                CStr::from_ptr(gl.GetString(ffi::VERSION) as *const c_char)
            );
            info!(
                log,
                "GL Vendor: {:?}",
                CStr::from_ptr(gl.GetString(ffi::VENDOR) as *const c_char)
            );
            info!(
                log,
                "GL Renderer: {:?}",
                CStr::from_ptr(gl.GetString(ffi::RENDERER) as *const c_char)
            );
            info!(log, "Supported GL Extensions: {:?}", exts);

            let gl_version = version::GlVersion::try_from(&gl).unwrap_or_else(|_| {
                warn!(log, "Failed to detect GLES version, defaulting to 2.0");
                version::GLES_2_0
            });

            // required for the manditory wl_shm formats
            if !exts.iter().any(|ext| ext == "GL_EXT_texture_format_BGRA8888") {
                return Err(Gles2Error::GLExtensionNotSupported(&[
                    "GL_EXT_texture_format_BGRA8888",
                ]));
            }
            // required for buffers without linear memory layout
            if gl_version < version::GLES_3_0 && !exts.iter().any(|ext| ext == "GL_EXT_unpack_subimage") {
                return Err(Gles2Error::GLExtensionNotSupported(&["GL_EXT_unpack_subimage"]));
            }

            let logger = if exts.iter().any(|ext| ext == "GL_KHR_debug") {
                let logger = Box::into_raw(Box::new(log.clone()));
                gl.Enable(ffi::DEBUG_OUTPUT);
                gl.Enable(ffi::DEBUG_OUTPUT_SYNCHRONOUS);
                gl.DebugMessageCallback(Some(gl_debug_log), logger as *mut nix::libc::c_void);
                Some(logger)
            } else {
                None
            };

            (gl, exts, logger)
        };

        let programs = [
            texture_program(&gl, shaders::FRAGMENT_SHADER_ABGR)?,
            texture_program(&gl, shaders::FRAGMENT_SHADER_XBGR)?,
            texture_program(&gl, shaders::FRAGMENT_SHADER_EXTERNAL)?,
        ];

        let (tx, rx) = channel();
        let renderer = Gles2Renderer {
            id: RENDERER_COUNTER.fetch_add(1, Ordering::SeqCst),
            gl,
            egl: context,
            extensions: exts,
            programs,
            target_buffer: None,
            target_surface: None,
            buffers: Vec::new(),
            current_projection: None,
            destruction_callback: rx,
            destruction_callback_sender: tx,
            logger,
            _not_send: std::ptr::null_mut(),
        };
        renderer.egl.unbind()?;
        Ok(renderer)
    }

    fn make_current(&self) -> Result<(), MakeCurrentError> {
        unsafe {
            if let Some(surface) = self.target_surface.as_ref() {
                self.egl.make_current_with_surface(surface)?;
            } else {
                self.egl.make_current()?;
            }
        }
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), Gles2Error> {
        self.make_current()?;
        for texture in self.destruction_callback.try_iter() {
            unsafe {
                self.gl.DeleteTextures(1, &texture);
            }
        }
        Ok(())
    }
}

struct BufferCache {
    cache: Vec<Option<BufferCacheVariant>>,
}

enum BufferCacheVariant {
    Egl(Option<EGLBuffer>),
}

struct SurfaceCache {
    texture: Vec<Option<Gles2Texture>>,
}

impl Gles2Renderer {
    #[cfg(feature = "wayland_frontend")]
    fn import_shm(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        cache: &mut Option<Gles2Texture>,
    ) -> Result<Gles2Texture, Gles2Error> {
        use crate::wayland::shm::with_buffer_contents;

        with_buffer_contents(&buffer, |slice, data| {
            self.make_current()?;

            let offset = data.offset as i32;
            let width = data.width as i32;
            let height = data.height as i32;
            let stride = data.stride as i32;

            // number of bytes per pixel
            // TODO: compute from data.format
            let pixelsize = 4i32;

            // ensure consistency, the SHM handler of smithay should ensure this
            assert!((offset + (height - 1) * stride + width * pixelsize) as usize <= slice.len());

            let (gl_format, shader_idx) = match data.format {
                wl_shm::Format::Abgr8888 => (ffi::RGBA, 0),
                wl_shm::Format::Xbgr8888 => (ffi::RGBA, 1),
                wl_shm::Format::Argb8888 => (ffi::BGRA_EXT, 0),
                wl_shm::Format::Xrgb8888 => (ffi::BGRA_EXT, 1),
                format => return Err(Gles2Error::UnsupportedPixelFormat(format)),
            };

            let texture = cache.as_ref().cloned().unwrap_or_else(|| {
                let mut tex = 0;
                unsafe { self.gl.GenTextures(1, &mut tex) };
                Gles2Texture(Rc::new(Gles2TextureInternal {
                    texture: tex,
                    texture_kind: shader_idx,
                    is_external: false,
                    y_inverted: false,
                    width: width as u32,
                    height: height as u32,
                    destruction_callback_sender: self.destruction_callback_sender.clone(),
                }))
            });

            unsafe {
                self.gl.BindTexture(ffi::TEXTURE_2D, texture.0.texture);

                self.gl
                    .TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
                self.gl
                    .TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
                self.gl.PixelStorei(ffi::UNPACK_ROW_LENGTH, stride / pixelsize);
                self.gl.TexImage2D(
                    ffi::TEXTURE_2D,
                    0,
                    gl_format as i32,
                    width,
                    height,
                    0,
                    gl_format,
                    ffi::UNSIGNED_BYTE as u32,
                    slice.as_ptr().offset(offset as isize) as *const _,
                );

                self.gl.PixelStorei(ffi::UNPACK_ROW_LENGTH, 0);
                self.gl.BindTexture(ffi::TEXTURE_2D, 0);
            }

            self.egl.unbind()?;

            *cache = Some(texture.clone());
            Ok(texture)
        })
        .map_err(Gles2Error::BufferAccessError)?
    }

    #[cfg(feature = "wayland_frontend")]
    fn import_egl(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        reader: &EGLBufferReader,
        buffer_cache: &mut Option<EGLBuffer>,
        texture_cache: &mut Option<Gles2Texture>,
    ) -> Result<Gles2Texture, Gles2Error> {
        if !self.extensions.iter().any(|ext| ext == "GL_OES_EGL_image") {
            return Err(Gles2Error::GLExtensionNotSupported(&["GL_OES_EGL_image"]));
        }

        self.make_current()?;
        let old_buffer = buffer_cache.take();
        let new_buffer = reader
            .egl_buffer_contents(&buffer)
            .map_err(Gles2Error::EGLBufferAccessError)?;

        // we do not need to re-import external textures
        if let Some(old_buffer) = old_buffer {
            if old_buffer.format == EGLFormat::External
                && new_buffer.format == EGLFormat::External
                && old_buffer.image(0) == new_buffer.image(0)
            // good enough
            {
                if let Some(texture) = texture_cache.as_ref().cloned() {
                    *buffer_cache = Some(new_buffer);
                    return Ok(texture);
                }
            }
        }

        let tex = self.import_egl_image(
            new_buffer.image(0).unwrap(),
            new_buffer.format == EGLFormat::External,
            texture_cache.as_ref().map(|x| x.0.texture),
        )?;
        let texture = texture_cache.as_ref().cloned().unwrap_or_else(|| {
            Gles2Texture(Rc::new(Gles2TextureInternal {
                texture: tex,
                texture_kind: match new_buffer.format {
                    EGLFormat::RGB => 1,
                    EGLFormat::RGBA => 0,
                    EGLFormat::External => 2,
                    _ => unreachable!("EGLBuffer currenly does not expose multi-planar buffers to us"),
                },
                is_external: new_buffer.format == EGLFormat::External,
                y_inverted: new_buffer.y_inverted,
                width: new_buffer.width,
                height: new_buffer.height,
                destruction_callback_sender: self.destruction_callback_sender.clone(),
            }))
        });
        self.egl.unbind()?;

        *buffer_cache = Some(new_buffer);
        *texture_cache = Some(texture.clone());
        Ok(texture)
    }

    fn import_egl_image(
        &self,
        image: EGLImage,
        is_external: bool,
        tex: Option<u32>,
    ) -> Result<u32, Gles2Error> {
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
        }

        Ok(tex)
    }
}

impl Bind<Rc<EGLSurface>> for Gles2Renderer {
    fn bind(&mut self, surface: Rc<EGLSurface>) -> Result<(), Gles2Error> {
        self.unbind()?;
        self.target_surface = Some(surface);
        Ok(())
    }
}

impl Bind<Dmabuf> for Gles2Renderer {
    fn bind(&mut self, dmabuf: Dmabuf) -> Result<(), Gles2Error> {
        self.unbind()?;
        unsafe {
            self.egl.make_current()?;
        }

        // Free outdated buffer resources
        // TODO: Replace with `drain_filter` once it lands
        let mut i = 0;
        while i != self.buffers.len() {
            if self.buffers[i].dmabuf.upgrade().is_none() {
                self.buffers.remove(i);
            } else {
                i += 1;
            }
        }

        let buffer = self
            .buffers
            .iter()
            .find(|buffer| {
                if let Some(dma) = buffer.dmabuf.upgrade() {
                    dma == dmabuf
                } else {
                    false
                }
            })
            .map(|buf| {
                let dmabuf = buf
                    .dmabuf
                    .upgrade()
                    .expect("Dmabuf equal check succeeded for freed buffer");
                Ok(Gles2Buffer {
                    internal: buf.clone(),
                    // we keep the dmabuf alive as long as we are bound
                    _dmabuf: dmabuf,
                })
            })
            .unwrap_or_else(|| {
                let image = self
                    .egl
                    .display
                    .create_image_from_dmabuf(&dmabuf)
                    .map_err(Gles2Error::BindBufferEGLError)?;

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
                        return Err(Gles2Error::FramebufferBindingError);
                    }

                    let weak = WeakGles2Buffer {
                        dmabuf: dmabuf.weak(),
                        image,
                        rbo,
                        fbo,
                    };

                    self.buffers.push(weak.clone());

                    Ok(Gles2Buffer {
                        internal: weak,
                        _dmabuf: dmabuf,
                    })
                }
            })?;

        unsafe {
            self.gl.BindFramebuffer(ffi::FRAMEBUFFER, buffer.internal.fbo);
        }

        self.target_buffer = Some(buffer);
        Ok(())
    }

    fn supported_formats(&self) -> Option<HashSet<Format>> {
        Some(self.egl.display.dmabuf_render_formats.clone())
    }
}

impl Unbind for Gles2Renderer {
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error> {
        unsafe {
            self.egl.make_current()?;
        }
        unsafe { self.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0) };
        self.target_buffer = None;
        self.target_surface = None;
        self.egl.unbind()?;
        Ok(())
    }
}

impl Drop for Gles2Renderer {
    fn drop(&mut self) {
        unsafe {
            if self.egl.make_current().is_ok() {
                self.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
                for program in &self.programs {
                    self.gl.DeleteProgram(program.program);
                }

                if self.extensions.iter().any(|ext| ext == "GL_KHR_debug") {
                    self.gl.Disable(ffi::DEBUG_OUTPUT);
                    self.gl.DebugMessageCallback(None, ptr::null());
                }
                if let Some(logger) = self.logger {
                    let _ = Box::from_raw(logger);
                }

                let _ = self.egl.unbind();
            }
        }
    }
}

static VERTS: [ffi::types::GLfloat; 8] = [
    1.0, 0.0, // top right
    0.0, 0.0, // top left
    1.0, 1.0, // bottom right
    0.0, 1.0, // bottom left
];

static TEX_COORDS: [ffi::types::GLfloat; 8] = [
    1.0, 0.0, // top right
    0.0, 0.0, // top left
    1.0, 1.0, // bottom right
    0.0, 1.0, // bottom left
];

impl Renderer for Gles2Renderer {
    type Error = Gles2Error;
    type TextureId = Gles2Texture;

    #[cfg(feature = "wayland_frontend")]
    fn shm_formats(&self) -> &[wl_shm::Format] {
        &[
            wl_shm::Format::Abgr8888,
            wl_shm::Format::Xbgr8888,
            wl_shm::Format::Argb8888,
            wl_shm::Format::Xrgb8888,
        ]
    }

    #[cfg(feature = "image")]
    fn import_bitmap<C: std::ops::Deref<Target = [u8]>>(
        &mut self,
        image: &image::ImageBuffer<image::Rgba<u8>, C>,
    ) -> Result<Self::TextureId, Self::Error> {
        self.make_current()?;

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
                ffi::RGBA as i32,
                image.width() as i32,
                image.height() as i32,
                0,
                ffi::RGBA,
                ffi::UNSIGNED_BYTE as u32,
                image.as_ptr() as *const _,
            );
            self.gl.BindTexture(ffi::TEXTURE_2D, 0);
        }

        let texture = Gles2Texture(Rc::new(Gles2TextureInternal {
            texture: tex,
            texture_kind: 2,
            is_external: false,
            y_inverted: false,
            width: image.width(),
            height: image.height(),
            destruction_callback_sender: self.destruction_callback_sender.clone(),
        }));
        self.egl.unbind()?;

        Ok(texture)
    }

    #[cfg(feature = "wayland_frontend")]
    fn import_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceAttributes>,
        egl: Option<&EGLBufferReader>,
    ) -> Result<Self::TextureId, Self::Error> {
        // init cache if not existing
        let buffer_cell = match buffer.as_ref().user_data().get::<Rc<RefCell<BufferCache>>>() {
            Some(cache) => cache.clone(),
            None => {
                let cache = BufferCache {
                    cache: Vec::with_capacity(self.id + 1),
                };

                let data: Rc<RefCell<BufferCache>> = Rc::new(RefCell::new(cache));
                let result = data.clone();
                buffer.as_ref().user_data().set(|| data);
                result
            }
        };
        let mut cache = buffer_cell.borrow_mut();
        while cache.cache.len() <= self.id {
            cache.cache.push(None);
        }

        if let Some(attributes) = surface {
            let texture_cell = match attributes.user_data.get::<Rc<RefCell<SurfaceCache>>>() {
                Some(cache) => cache.clone(),
                None => {
                    let cache = SurfaceCache {
                        texture: Vec::with_capacity(self.id + 1),
                    };

                    let data: Rc<RefCell<SurfaceCache>> = Rc::new(RefCell::new(cache));
                    let result = data.clone();
                    attributes.user_data.insert_if_missing(|| data);
                    result
                }
            };
            let mut cache = texture_cell.borrow_mut();
            while cache.texture.len() <= self.id {
                cache.texture.push(None);
            }
        }

        // init buffer cache variants
        if cache.cache[self.id].is_none() {
            if egl.and_then(|egl| egl.egl_buffer_dimensions(&buffer)).is_some() {
                cache.cache[self.id] = Some(BufferCacheVariant::Egl(None));
            }
        }

        // delegate for different buffer types
        let mut texture_cache_tmp = surface
            .and_then(|a| a.user_data.get::<Rc<RefCell<SurfaceCache>>>())
            .map(|cache| cache.borrow_mut());
        let mut temporary_none = None;
        let texture_cache = texture_cache_tmp
            .as_mut()
            .map(|cache| &mut cache.texture[self.id])
            .unwrap_or(&mut temporary_none);
        if egl.and_then(|egl| egl.egl_buffer_dimensions(&buffer)).is_some() {
            let buffer_cache = match &mut cache.cache[self.id] {
                Some(BufferCacheVariant::Egl(cache)) => cache,
                _ => unreachable!(),
            };
            self.import_egl(&buffer, egl.unwrap(), buffer_cache, texture_cache)
        } else if crate::wayland::shm::with_buffer_contents(&buffer, |_, _| ()).is_ok() {
            self.import_shm(&buffer, texture_cache)
        } else {
            Err(Gles2Error::UnknownBufferType)
        }
    }

    fn begin(&mut self, width: u32, height: u32, transform: Transform) -> Result<(), Gles2Error> {
        self.make_current()?;
        // delayed destruction until the next frame rendering.
        self.cleanup()?;

        unsafe {
            self.gl.Viewport(0, 0, width as i32, height as i32);

            self.gl.Enable(ffi::BLEND);
            self.gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
        }

        // replicate https://www.khronos.org/registry/OpenGL-Refpages/gl2.1/xhtml/glOrtho.xml
        // glOrtho(0, width, 0, height, 1, 1);
        let mut renderer = Matrix3::<f32>::identity();
        let t = Matrix3::<f32>::identity();
        let x = 2.0 / (width as f32);
        let y = 2.0 / (height as f32);

        // Rotation & Reflection
        renderer[0][0] = x * t[0][0];
        renderer[1][0] = x * t[0][1];
        renderer[0][1] = y * -t[1][0];
        renderer[1][1] = y * -t[1][1];

        //Translation
        renderer[2][0] = -(1.0f32.copysign(renderer[0][0] + renderer[1][0]));
        renderer[2][1] = -(1.0f32.copysign(renderer[0][1] + renderer[1][1]));

        // output transformation passed in by the user
        self.current_projection = Some(transform.matrix() * renderer);
        Ok(())
    }

    fn clear(&mut self, color: [f32; 4]) -> Result<(), Self::Error> {
        self.make_current()?;
        unsafe {
            self.gl.ClearColor(color[0], color[1], color[2], color[3]);
            self.gl.Clear(ffi::COLOR_BUFFER_BIT);
        }

        Ok(())
    }

    fn render_texture(
        &mut self,
        tex: &Self::TextureId,
        mut matrix: Matrix3<f32>,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        self.make_current()?;
        if self.current_projection.is_none() {
            return Err(Gles2Error::UnconstraintRenderingOperation);
        }

        //apply output transformation
        matrix = self.current_projection.as_ref().unwrap() * matrix;

        let target = if tex.0.is_external {
            ffi::TEXTURE_EXTERNAL_OES
        } else {
            ffi::TEXTURE_2D
        };

        // render
        unsafe {
            self.gl.ActiveTexture(ffi::TEXTURE0);
            self.gl.BindTexture(target, tex.0.texture);
            self.gl
                .TexParameteri(target, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
            self.gl.UseProgram(self.programs[tex.0.texture_kind].program);

            self.gl
                .Uniform1i(self.programs[tex.0.texture_kind].uniform_tex, 0);
            self.gl.UniformMatrix3fv(
                self.programs[tex.0.texture_kind].uniform_matrix,
                1,
                ffi::FALSE,
                matrix.as_ptr(),
            );
            self.gl.Uniform1i(
                self.programs[tex.0.texture_kind].uniform_invert_y,
                if tex.0.y_inverted { 1 } else { 0 },
            );
            self.gl
                .Uniform1f(self.programs[tex.0.texture_kind].uniform_alpha, alpha);

            self.gl.VertexAttribPointer(
                self.programs[tex.0.texture_kind].attrib_position as u32,
                2,
                ffi::FLOAT,
                ffi::FALSE,
                0,
                VERTS.as_ptr() as *const _,
            );
            self.gl.VertexAttribPointer(
                self.programs[tex.0.texture_kind].attrib_tex_coords as u32,
                2,
                ffi::FLOAT,
                ffi::FALSE,
                0,
                TEX_COORDS.as_ptr() as *const _,
            );

            self.gl
                .EnableVertexAttribArray(self.programs[tex.0.texture_kind].attrib_position as u32);
            self.gl
                .EnableVertexAttribArray(self.programs[tex.0.texture_kind].attrib_tex_coords as u32);

            self.gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);

            self.gl
                .DisableVertexAttribArray(self.programs[tex.0.texture_kind].attrib_position as u32);
            self.gl
                .DisableVertexAttribArray(self.programs[tex.0.texture_kind].attrib_tex_coords as u32);

            self.gl.BindTexture(target, 0);
        }

        Ok(())
    }

    fn finish(&mut self) -> Result<(), crate::backend::SwapBuffersError> {
        self.make_current()?;
        unsafe {
            self.gl.Flush();
            self.gl.Disable(ffi::BLEND);
        }

        self.current_projection = None;

        Ok(())
    }
}
