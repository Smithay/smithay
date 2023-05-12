/// OpenGL Shaders with support for color transformations
use crate::backend::renderer::gles::*;

pub(in super::super) const OUTPUT_VERTEX_SHADER: &str = include_str!("./output.vert");
pub(in super::super) const OUTPUT_FRAGMENT_SHADER: &str = include_str!("./output.frag");

#[derive(Debug)]
pub(in super::super) struct GlesColorOutputProgram {
    pub(in super::super) program: ffi::types::GLuint,
    pub(in super::super) attrib_vert: ffi::types::GLint,
    pub(in super::super) uniform_tex: ffi::types::GLint,
    pub(super) destruction_callback_sender: Sender<CleanupResource>,
}

impl Drop for GlesColorOutputProgram {
    fn drop(&mut self) {
        let _ = self
            .destruction_callback_sender
            .send(CleanupResource::Program(self.program));
    }
}
