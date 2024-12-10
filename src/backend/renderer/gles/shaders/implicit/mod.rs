/// OpenGL Shaders
use crate::backend::renderer::gles::*;

pub(in super::super) const VERTEX_SHADER: &str = include_str!("./texture.vert");
pub(in super::super) const FRAGMENT_SHADER: &str = include_str!("./texture.frag");

pub(in super::super) const VERTEX_SHADER_SOLID: &str = include_str!("./solid.vert");
pub(in super::super) const FRAGMENT_SHADER_SOLID: &str = include_str!("./solid.frag");

#[derive(Debug)]
pub(in super::super) struct GlesTexProgramInternal {
    pub(in super::super) program: ffi::types::GLuint,
    pub(in super::super) uniform_tex: ffi::types::GLint,
    pub(in super::super) uniform_tex_matrix: ffi::types::GLint,
    pub(in super::super) uniform_matrix: ffi::types::GLint,
    pub(in super::super) uniform_alpha: ffi::types::GLint,
    pub(in super::super) attrib_vert: ffi::types::GLint,
    pub(in super::super) attrib_vert_position: ffi::types::GLint,
    pub(in super::super) additional_uniforms: HashMap<String, UniformDesc>,
}

#[derive(Debug)]
pub(in super::super) struct GlesTexProgramVariant {
    pub(in super::super) normal: GlesTexProgramInternal,
    pub(in super::super) debug: GlesTexProgramInternal,

    // debug flags
    pub(in super::super) uniform_tint: ffi::types::GLint,
}

/// Gles texture shader
///
/// The program can be used with the same [`GlesRenderer`] it was created with, or one using a
/// shared [`EGLContext`].
#[derive(Debug, Clone)]
pub struct GlesTexProgram(pub(in super::super) Arc<GlesTexProgramInner>);

#[derive(Debug)]
pub(in super::super) struct GlesTexProgramInner {
    pub(in super::super) variants: [GlesTexProgramVariant; 3],
    pub(super) destruction_callback_sender: Sender<CleanupResource>,
}

impl GlesTexProgram {
    pub(in super::super) fn variant_for_format(
        &self,
        format: Option<ffi::types::GLenum>,
        has_alpha: bool,
    ) -> &GlesTexProgramVariant {
        match format {
            Some(ffi::BGRA_EXT) | Some(ffi::RGBA) | Some(ffi::RGBA8) | Some(ffi::RGB10_A2)
            | Some(ffi::RGBA16F) => {
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
pub(in super::super) struct GlesSolidProgram {
    pub(in super::super) program: ffi::types::GLuint,
    pub(in super::super) uniform_matrix: ffi::types::GLint,
    pub(in super::super) uniform_color: ffi::types::GLint,
    pub(in super::super) attrib_vert: ffi::types::GLint,
    pub(in super::super) attrib_position: ffi::types::GLint,
}

/// Gles pixel shader
///
/// The program can be used with the same [`GlesRenderer`] it was created with, or one using a
/// shared [`EGLContext`].
#[derive(Debug, Clone)]
pub struct GlesPixelProgram(pub(in super::super) Arc<GlesPixelProgramInner>);

#[derive(Debug)]
pub(in super::super) struct GlesPixelProgramInner {
    pub(in super::super) normal: GlesPixelProgramInternal,
    pub(in super::super) debug: GlesPixelProgramInternal,
    pub(in super::super) destruction_callback_sender: Sender<CleanupResource>,

    // debug flags
    pub(in super::super) uniform_tint: ffi::types::GLint,
}

#[derive(Debug)]
pub(in super::super) struct GlesPixelProgramInternal {
    pub(in super::super) program: ffi::types::GLuint,
    pub(in super::super) uniform_matrix: ffi::types::GLint,
    pub(in super::super) uniform_tex_matrix: ffi::types::GLint,
    pub(in super::super) uniform_size: ffi::types::GLint,
    pub(in super::super) uniform_alpha: ffi::types::GLint,
    pub(in super::super) attrib_vert: ffi::types::GLint,
    pub(in super::super) attrib_position: ffi::types::GLint,
    pub(in super::super) additional_uniforms: HashMap<String, UniformDesc>,
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
