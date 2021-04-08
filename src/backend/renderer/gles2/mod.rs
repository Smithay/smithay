use std::collections::HashSet;
use std::ffi::CStr;
use std::ptr;
use std::sync::Arc;

use cgmath::{prelude::*, Matrix3, Vector2};

mod shaders;
use crate::backend::allocator::{dmabuf::{Dmabuf, WeakDmabuf}, Format};
use crate::backend::egl::{EGLContext, EGLSurface, ffi::egl::types::EGLImage};
use super::{Renderer, Frame, Bind, Unbind, Transform, Texture};

#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::{wl_shm, wl_buffer};

#[allow(clippy::all, missing_docs)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/gl_bindings.rs"));
}

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

pub struct Gles2Texture {
    texture: ffi::types::GLuint,
    texture_kind: usize,
    is_external: bool,
    y_inverted: bool,
    width: u32,
    height: u32,
}

impl Texture for Gles2Texture {
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
}

#[derive(Clone)]
struct WeakGles2Buffer {
    dmabuf: WeakDmabuf,
    image: EGLImage,
    rbo: ffi::types::GLuint,
    fbo: ffi::types::GLuint,
}

struct Gles2Buffer {
    internal: WeakGles2Buffer,
    _dmabuf: Dmabuf,
}

pub struct Gles2Renderer {
    internal: Arc<Gles2RendererInternal>,
    buffers: Vec<WeakGles2Buffer>,
    current_buffer: Option<Gles2Buffer>,
}

struct Gles2RendererInternal {
    gl: ffi::Gles2,
    egl: EGLContext,
    extensions: Vec<String>,
    programs: [Gles2Program; shaders::FRAGMENT_COUNT],
    logger: Option<*mut ::slog::Logger>,
}

pub struct Gles2Frame {
    internal: Arc<Gles2RendererInternal>,
    projection: Matrix3<f32>,
}

#[derive(thiserror::Error, Debug)]
pub enum Gles2Error {
    #[error("Failed to compile Shader: {0}")]
    ShaderCompileError(&'static str),
    #[error("Failed to link Program")]
    ProgramLinkError,
    #[error("Failed to bind Framebuffer")]
    FramebufferBindingError,
    #[error("Failed to load GL functions from EGL")]
    GLFunctionLoaderError,
    /// The required GL extension is not supported by the underlying implementation
    #[error("None of the following GL extensions is supported by the underlying GL implementation, at least one is required: {0:?}")]
    GLExtensionNotSupported(&'static [&'static str]),
    #[error("Failed to active egl context")]
    ContextActivationError(#[from] crate::backend::egl::MakeCurrentError),
    #[error("Failed to convert dmabuf to EGLImage")]
    BindBufferEGLError(#[source] crate::backend::egl::Error),
    #[error("Unsupported pixel format: {0:?}")]
    #[cfg(feature = "wayland_frontend")]
    UnsupportedPixelFormat(wl_shm::Format),
    #[error("Error accessing the buffer ({0:?})")]
    #[cfg(feature = "wayland_frontend")]
    BufferAccessError(crate::wayland::shm::BufferAccessError),
}

extern "system" fn gl_debug_log(_source: ffi::types::GLenum,
                           gltype: ffi::types::GLenum,
                           _id: ffi::types::GLuint,
                           _severity: ffi::types::GLenum,
                           _length: ffi::types::GLsizei,
                           message: *const ffi::types::GLchar,
                           user_param: *mut nix::libc::c_void)
{
    let _ = std::panic::catch_unwind(move || {
        unsafe {
            let msg = CStr::from_ptr(message);
            let log = Box::from_raw(user_param as *mut ::slog::Logger);
            let message_utf8 = msg.to_string_lossy();    
            match gltype {
                ffi::DEBUG_TYPE_ERROR | ffi::DEBUG_TYPE_UNDEFINED_BEHAVIOR => error!(log, "[GL] {}", message_utf8),
                ffi::DEBUG_TYPE_DEPRECATED_BEHAVIOR => warn!(log, "[GL] {}", message_utf8),
                _ => debug!(log, "[GL] {}", message_utf8),
            };
            std::mem::forget(log);
        }
    });
}

unsafe fn compile_shader(gl: &ffi::Gles2, variant: ffi::types::GLuint, src: &'static str) -> Result<ffi::types::GLuint, Gles2Error> {
    let shader = gl.CreateShader(variant);
    gl.ShaderSource(shader, 1, &src.as_ptr() as *const *const u8 as *const *const i8, &(src.len() as i32) as *const _);
    gl.CompileShader(shader);

    let mut status = ffi::FALSE as i32;
    gl.GetShaderiv(shader, ffi::COMPILE_STATUS, &mut status as *mut _);
    if status == ffi::FALSE as i32 {
        gl.DeleteShader(shader);
        return Err(Gles2Error::ShaderCompileError(src));
    }

    Ok(shader)
}

unsafe fn link_program(gl: &ffi::Gles2, vert_src: &'static str, frag_src: &'static str) -> Result<ffi::types::GLuint, Gles2Error> {
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
        uniform_tex: gl.GetUniformLocation(program, tex.as_ptr() as *const i8),
        uniform_matrix: gl.GetUniformLocation(program, matrix.as_ptr() as *const i8),
        uniform_invert_y: gl.GetUniformLocation(program, invert_y.as_ptr() as *const i8),
        uniform_alpha: gl.GetUniformLocation(program, alpha.as_ptr() as *const i8),
        attrib_position: gl.GetAttribLocation(program, position.as_ptr() as *const i8),
        attrib_tex_coords: gl.GetAttribLocation(program, tex_coords.as_ptr() as *const i8),
    })
}

impl Gles2Renderer {
    pub fn new<L>(context: EGLContext, logger: L) -> Result<Gles2Renderer, Gles2Error>
    where
        L: Into<Option<::slog::Logger>>
    {
        let log = crate::slog_or_fallback(logger).new(o!("smithay_module" => "renderer_gles2"));

        unsafe { context.make_current()? };

        let (gl, exts, logger) = unsafe {
            let gl = ffi::Gles2::load_with(|s| crate::backend::egl::get_proc_address(s) as *const _);
            let ext_ptr = gl.GetString(ffi::EXTENSIONS) as *const i8;
            if ext_ptr.is_null() {
                return Err(Gles2Error::GLFunctionLoaderError);
            }

            let exts = {
                let p = CStr::from_ptr(ext_ptr);
                let list = String::from_utf8(p.to_bytes().to_vec()).unwrap_or_else(|_| String::new());
                list.split(' ').map(|e| e.to_string()).collect::<Vec<_>>()
            };

            info!(log, "Initializing OpenGL ES Renderer");
            info!(log, "GL Version: {:?}", CStr::from_ptr(gl.GetString(ffi::VERSION) as *const i8));
            info!(log, "GL Vendor: {:?}", CStr::from_ptr(gl.GetString(ffi::VENDOR) as *const i8));
            info!(log, "GL Renderer: {:?}", CStr::from_ptr(gl.GetString(ffi::RENDERER) as *const i8));
            info!(log, "Supported GL Extensions: {:?}", exts);

            // required for the manditory wl_shm formats
            if !exts.iter().any(|ext| ext == "GL_EXT_texture_format_BGRA8888") {
                return Err(Gles2Error::GLExtensionNotSupported(&["GL_EXT_texture_format_BGRA8888"]));
            }
            // required for buffers without linear memory layout
            if !exts.iter().any(|ext| ext == "GL_EXT_unpack_subimage") {
                return Err(Gles2Error::GLExtensionNotSupported(&["GL_EXT_unpack_subimage"]));
            }

            let logger = if exts.iter().any(|ext| ext == "GL_KHR_debug") {
                let logger = Box::into_raw(Box::new(log.clone()));
                gl.Enable(ffi::DEBUG_OUTPUT);
                gl.Enable(ffi::DEBUG_OUTPUT_SYNCHRONOUS);
                gl.DebugMessageCallback(Some(gl_debug_log), logger as *mut nix::libc::c_void);
                Some(logger)
            } else { None };

            (gl, exts, logger)
        };

        let programs =  {
            unsafe { [
                texture_program(&gl, shaders::FRAGMENT_SHADER_ABGR)?,
                texture_program(&gl, shaders::FRAGMENT_SHADER_XBGR)?,
                texture_program(&gl, shaders::FRAGMENT_SHADER_BGRA)?,
                texture_program(&gl, shaders::FRAGMENT_SHADER_BGRX)?,
                texture_program(&gl, shaders::FRAGMENT_SHADER_EXTERNAL)?,
            ] }
        };

        Ok(Gles2Renderer {
            internal: Arc::new(Gles2RendererInternal {
                gl,
                egl: context,
                extensions: exts,
                programs,
                logger,
            }),
            buffers: Vec::new(),
            current_buffer: None,
        })
    }
}

impl Bind<&EGLSurface> for Gles2Renderer {
    fn bind(&mut self, surface: &EGLSurface) -> Result<(), Gles2Error> {
        if self.current_buffer.is_some() {
            self.unbind()?;
        }

        unsafe {
            self.internal.egl.make_current_with_surface(&surface)?;
        }

        Ok(())
    }
}

impl Bind<Dmabuf> for Gles2Renderer {
    fn bind(&mut self, dmabuf: Dmabuf) -> Result<(), Gles2Error> {
        if self.current_buffer.is_some() {
            self.unbind()?;
        }

        unsafe {
            self.internal.egl.make_current()?;
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

        let buffer = self.buffers
            .iter()
            .find(|buffer| dmabuf == buffer.dmabuf)
            .map(|buf| {
                let dmabuf = buf.dmabuf.upgrade().expect("Dmabuf equal check succeeded for freed buffer");
                Ok(Gles2Buffer {
                    internal: buf.clone(),
                    // we keep the dmabuf alive as long as we are bound
                    _dmabuf: dmabuf
                })
            })
            .unwrap_or_else(|| {
                let image = self.internal.egl.display.create_image_from_dmabuf(&dmabuf).map_err(Gles2Error::BindBufferEGLError)?;

                unsafe {
                    let mut rbo = 0;
                    self.internal.gl.GenRenderbuffers(1, &mut rbo as *mut _);
                    self.internal.gl.BindRenderbuffer(ffi::RENDERBUFFER, rbo);
                    self.internal.gl.EGLImageTargetRenderbufferStorageOES(ffi::RENDERBUFFER, image);
                    self.internal.gl.BindRenderbuffer(ffi::RENDERBUFFER, 0);

                    let mut fbo = 0;
                    self.internal.gl.GenFramebuffers(1, &mut fbo as *mut _);
                    self.internal.gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo);
                    self.internal.gl.FramebufferRenderbuffer(ffi::FRAMEBUFFER, ffi::COLOR_ATTACHMENT0, ffi::RENDERBUFFER, rbo);
                    let status = self.internal.gl.CheckFramebufferStatus(ffi::FRAMEBUFFER);
                    self.internal.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);

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
                        _dmabuf: dmabuf
                    })
                }
            })?;

        unsafe {
            self.internal.gl.BindFramebuffer(ffi::FRAMEBUFFER, buffer.internal.fbo);
        }
        self.current_buffer = Some(buffer);
        Ok(())
    }

    fn supported_formats(&self) -> Option<HashSet<Format>> {
        Some(self.internal.egl.display.dmabuf_render_formats.clone())
    }
}

impl Unbind for Gles2Renderer {
    fn unbind(&mut self) -> Result<(), Gles2Error> {
        unsafe {
            self.internal.egl.make_current()?;
            self.internal.gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
        }
        self.current_buffer = None;
        let _ = self.internal.egl.unbind();
        Ok(())
    }
}

impl Renderer for Gles2Renderer {
    type Error = Gles2Error;
    type Texture = Gles2Texture;
    type Frame = Gles2Frame;
    
    fn begin(&mut self, width: u32, height: u32, transform: Transform) -> Result<Gles2Frame, Gles2Error> {
        if !self.internal.egl.is_current() {
            // Do not call this unconditionally.
            // If surfaces are in use (e.g. for winit) this would unbind them.
            unsafe { self.internal.egl.make_current()?; }
        }
        unsafe {
            self.internal.gl.Viewport(0, 0, width as i32, height as i32);
            
            self.internal.gl.Enable(ffi::BLEND);
            self.internal.gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
        }

        // output transformation passed in by the user
        let mut projection = Matrix3::<f32>::identity();
        projection = projection * Matrix3::from_translation(Vector2::new(width as f32 / 2.0, height as f32 / 2.0));
        projection = projection * transform.matrix();
        let (transformed_width, transformed_height) = transform.transform_size(width, height);
        projection = projection * Matrix3::from_translation(Vector2::new(-(transformed_width as f32) / 2.0, -(transformed_height as f32) / 2.0));
        
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

        Ok(Gles2Frame {
            internal: self.internal.clone(),
            projection: projection * renderer,
        })
    }

    #[cfg(feature = "wayland_frontend")]
    fn shm_formats(&self) -> &[wl_shm::Format] {
        &[
            wl_shm::Format::Abgr8888,
            wl_shm::Format::Xbgr8888,
            wl_shm::Format::Argb8888,
            wl_shm::Format::Xrgb8888,
        ]
    }

    #[cfg(feature = "wayland_frontend")]
    fn import_shm(&self, buffer: &wl_buffer::WlBuffer) -> Result<Self::Texture, Self::Error> {
        use crate::wayland::shm::with_buffer_contents;

        with_buffer_contents(&buffer, |slice, data| {
            if !self.internal.egl.is_current() {
                unsafe { self.internal.egl.make_current()?; }
            }
            
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
                wl_shm::Format::Argb8888 => (ffi::BGRA_EXT, 2),
                wl_shm::Format::Xrgb8888 => (ffi::BGRA_EXT, 3),
                format => return Err(Gles2Error::UnsupportedPixelFormat(format)),
            };
            
            let mut tex = 0;
            unsafe {
                self.internal.gl.GenTextures(1, &mut tex);
                self.internal.gl.BindTexture(ffi::TEXTURE_2D, tex);

                self.internal.gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
                self.internal.gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
                self.internal.gl.PixelStorei(ffi::UNPACK_ROW_LENGTH, stride / pixelsize);
                self.internal.gl.TexImage2D(ffi::TEXTURE_2D, 0, gl_format as i32, width, height, 0, gl_format, ffi::UNSIGNED_BYTE as u32, slice.as_ptr() as *const _);

                self.internal.gl.PixelStorei(ffi::UNPACK_ROW_LENGTH, 0);
                self.internal.gl.BindTexture(ffi::TEXTURE_2D, 0);
            }
            
            Ok(Gles2Texture {
                texture: tex,
                texture_kind: shader_idx,
                is_external: false,
                y_inverted: false,
                width: width as u32,
                height: height as u32,
            })
        }).map_err(Gles2Error::BufferAccessError)?
    }
}

impl Drop for Gles2RendererInternal {
    fn drop(&mut self) {
        unsafe {
            if self.egl.make_current().is_ok() {
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

impl Frame for Gles2Frame {
    type Error = Gles2Error;
    type Texture = Gles2Texture;

    fn clear(&mut self, color: [f32; 4]) -> Result<(), Self::Error> {
        unsafe {
            self.internal.gl.ClearColor(color[0], color[1], color[2], color[3]);
            self.internal.gl.Clear(ffi::COLOR_BUFFER_BIT);
        }

        Ok(())
    }

    fn render_texture(&mut self, tex: &Self::Texture, mut matrix: Matrix3<f32>, alpha: f32) -> Result<(), Self::Error> {
        //apply output transformation
        matrix = self.projection * matrix;

        let target = if tex.is_external { ffi::TEXTURE_EXTERNAL_OES } else { ffi::TEXTURE_2D };

        // render
        unsafe {
            self.internal.gl.ActiveTexture(ffi::TEXTURE0);
            self.internal.gl.BindTexture(target, tex.texture);
            self.internal.gl.TexParameteri(target, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
            self.internal.gl.UseProgram(self.internal.programs[tex.texture_kind].program);
            
            self.internal.gl.Uniform1i(self.internal.programs[tex.texture_kind].uniform_tex, 0);
            self.internal.gl.UniformMatrix3fv(self.internal.programs[tex.texture_kind].uniform_matrix, 1, ffi::FALSE, matrix.as_ptr());
            self.internal.gl.Uniform1i(self.internal.programs[tex.texture_kind].uniform_invert_y, if tex.y_inverted { 1 } else { 0 });
            self.internal.gl.Uniform1f(self.internal.programs[tex.texture_kind].uniform_alpha, alpha);
            
            self.internal.gl.VertexAttribPointer(self.internal.programs[tex.texture_kind].attrib_position as u32, 2, ffi::FLOAT, ffi::FALSE, 0, VERTS.as_ptr() as *const _);
            self.internal.gl.VertexAttribPointer(self.internal.programs[tex.texture_kind].attrib_tex_coords as u32, 2, ffi::FLOAT, ffi::FALSE, 0, TEX_COORDS.as_ptr() as *const _);
            
            self.internal.gl.EnableVertexAttribArray(self.internal.programs[tex.texture_kind].attrib_position as u32);
            self.internal.gl.EnableVertexAttribArray(self.internal.programs[tex.texture_kind].attrib_tex_coords as u32);
            
            self.internal.gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
            
            self.internal.gl.DisableVertexAttribArray(self.internal.programs[tex.texture_kind].attrib_position as u32);
            self.internal.gl.DisableVertexAttribArray(self.internal.programs[tex.texture_kind].attrib_tex_coords as u32);
            
            self.internal.gl.BindTexture(target, 0);
        }

        Ok(())
    }

    fn finish(self) -> Result<(), crate::backend::SwapBuffersError> {
        unsafe {
            self.internal.gl.Flush();
            self.internal.gl.Disable(ffi::BLEND);
        }

        Ok(())
    }
}
