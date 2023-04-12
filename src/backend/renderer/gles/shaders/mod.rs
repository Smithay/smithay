//! OpenGL ES Shaders
use super::*;
mod uniform;
pub use self::uniform::*;

use std::{
    any::TypeId,
    ffi::CString,
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};

const VERTEX_SHADER: &str = include_str!("./draw.vert");
const FRAGMENT_SHADER: &str = include_str!("./draw.frag");

pub trait GlesShaderSource {
    fn vertex(&self) -> &str;
    fn fragment(&self) -> &str;
}

pub(super) struct BuildinShader;
impl GlesShaderSource for BuildinShader {
    fn vertex(&self) -> &str {
        VERTEX_SHADER
    }

    fn fragment(&self) -> &str {
        FRAGMENT_SHADER
    }
}

pub struct ShaderFactory {
    pub(super) renderer_id: usize,
    pub(super) source: Box<dyn GlesShaderSource>,
    pub(super) source_hash: SourceHash,
    pub(super) cache: HashMap<ShaderSettings, Weak<GlesProgram>>,
    pub(super) additional_uniforms: Option<Vec<UniformName<'static>>>,
    pub(super) destruction_callback_sender: Sender<CleanupResource>,
}

impl fmt::Debug for ShaderFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShaderFactory")
            .field("renderer_id", &self.renderer_id)
            .field(
                "source",
                &format!(
                    "Source (frag: {:?}..., vertex: {:?}...)",
                    &self.source.fragment()[0..32],
                    &self.source.vertex()[0..32]
                ),
            )
            .field("source_hash", &self.source_hash)
            .field("cache", &self.cache)
            .field("additional_uniforms", &self.additional_uniforms)
            .field("destruction_callback_sender", &self.destruction_callback_sender)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SourceHash(u64);

impl ShaderFactory {
    pub(super) fn new<S: GlesShaderSource + 'static>(
        renderer_id: usize,
        source: S,
        additional_uniforms: Option<Vec<UniformName<'static>>>,
        destruction_callback_sender: Sender<CleanupResource>,
    ) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        TypeId::of::<S>().hash(&mut hasher);
        source.vertex().hash(&mut hasher);
        source.fragment().hash(&mut hasher);
        let source_hash = SourceHash(hasher.finish());

        ShaderFactory {
            renderer_id,
            source: Box::new(source) as Box<dyn GlesShaderSource>,
            source_hash,
            cache: HashMap::new(),
            additional_uniforms,
            destruction_callback_sender,
        }
    }

    pub(super) fn source_hash(&self) -> &SourceHash {
        &self.source_hash
    }

    pub(super) fn program_for_settings(
        &mut self,
        gl: &ffi::Gles2,
        settings: &ShaderSettings,
    ) -> Result<Arc<GlesProgram>, GlesError> {
        self.cache.retain(|_, program| program.upgrade().is_some());
        if let Some(program) = self.cache.get(settings) {
            Ok(program
                .upgrade()
                .expect("We just cleaned the cache of dead references"))
        } else {
            let program = Arc::new(GlesProgram::new_custom(
                gl,
                settings,
                &*self.source,
                self.additional_uniforms.as_deref().unwrap_or(&[]),
                self.destruction_callback_sender.clone(),
            )?);
            self.cache.insert(*settings, Arc::downgrade(&program));
            Ok(program)
        }
    }
}

#[derive(Debug)]
pub(super) struct GlesProgram {
    pub(super) program: ffi::types::GLuint,
    pub(super) attrib_locations: AttributeLocations,
    pub(super) uniform_locations: UniformLocations,
    pub(super) additional_uniforms: Option<HashMap<String, UniformDesc>>,
    pub(super) destruction_callback_sender: Sender<CleanupResource>,
}

impl GlesProgram {
    pub fn new_default(
        gl: &ffi::Gles2,
        settings: &ShaderSettings,
        destruction_callback_sender: Sender<CleanupResource>,
    ) -> Result<GlesProgram, GlesError> {
        Self::new_custom(gl, settings, &BuildinShader, &[], destruction_callback_sender)
    }

    pub fn new_custom(
        gl: &ffi::Gles2,
        settings: &ShaderSettings,
        source: &dyn GlesShaderSource,
        additional_uniforms: &[UniformName<'_>],
        destruction_callback_sender: Sender<CleanupResource>,
    ) -> Result<GlesProgram, GlesError> {
        let vert_src = source.vertex();
        let frag_src = settings.add_settings_to_src(source.fragment());
        let program = unsafe { link_program(gl, vert_src, &frag_src)? };

        let attrib_locations = AttributeLocations {
            vert: attrib_location(gl, program, b"vert\0"),
            vert_position: attrib_location(gl, program, b"vert_position\0"),
        };
        let mut uniform_locations = UniformLocations {
            matrix: uniform_location(gl, program, b"matrix\0"),
            tex_matrix: uniform_location(gl, program, b"tex_matrix\0"),
            alpha: uniform_location(gl, program, b"alpha\0"),
            ..Default::default()
        };

        if settings.debug {
            uniform_locations.tint = uniform_location(gl, program, b"tint\0");
        }

        match settings.variant {
            ShaderVariant::Solid => {
                uniform_locations.color = uniform_location(gl, program, b"color\0");
            }
            ShaderVariant::Rgba | ShaderVariant::Rgbx | ShaderVariant::External => {
                uniform_locations.tex = uniform_location(gl, program, b"tex\0");
            }
            _ => {}
        }

        if settings.pre_curve == CurveType::_3x1dLut {
            uniform_locations.color_pre_curve_lut_2d =
                uniform_location(gl, program, b"color_pre_curve_lut_2d\0");
            uniform_locations.color_pre_curve_lut_scale_offset =
                uniform_location(gl, program, b"color_pre_curve_lut_scale_offset\0");
        }
        match settings.mapping {
            MappingType::Identity => {}
            MappingType::_3dLut => {
                uniform_locations.color_mapping_lut_3d =
                    uniform_location(gl, program, b"color_mapping_lut_3d\0");
                uniform_locations.color_mapping_lut_scale_offset =
                    uniform_location(gl, program, b"color_mapping_lut_scale_offset\0");
            }
            MappingType::Matrix => {
                uniform_locations.color_mapping_matrix =
                    uniform_location(gl, program, b"color_mapping_matrix\0");
            }
        }
        if settings.post_curve == CurveType::_3x1dLut {
            uniform_locations.color_post_curve_lut_2d =
                uniform_location(gl, program, b"color_post_curve_lut_2d\0");
            uniform_locations.color_post_curve_lut_scale_offset =
                uniform_location(gl, program, b"color_post_curve_lut_scale_offset\0");
        }

        let additional_uniforms = (!additional_uniforms.is_empty()).then(|| {
            additional_uniforms
                .iter()
                .map(|uniform| {
                    let name = CString::new(uniform.name.as_bytes()).expect("Interior null in name");
                    let location =
                        unsafe { gl.GetUniformLocation(program, name.as_ptr() as *const ffi::types::GLchar) };
                    (
                        uniform.name.clone().into_owned(),
                        UniformDesc {
                            location,
                            type_: uniform.type_,
                        },
                    )
                })
                .collect()
        });

        Ok(GlesProgram {
            program,
            attrib_locations,
            uniform_locations,
            additional_uniforms,
            destruction_callback_sender,
        })
    }
}

fn attrib_location(gl: &ffi::Gles2, program: ffi::types::GLuint, name: &[u8]) -> ffi::types::GLint {
    unsafe {
        gl.GetAttribLocation(
            program,
            CStr::from_bytes_with_nul(name).expect("NULL terminated").as_ptr() as *const ffi::types::GLchar,
        )
    }
}

fn uniform_location(gl: &ffi::Gles2, program: ffi::types::GLuint, name: &[u8]) -> ffi::types::GLint {
    unsafe {
        gl.GetUniformLocation(
            program,
            CStr::from_bytes_with_nul(name).expect("NULL terminated").as_ptr() as *const ffi::types::GLchar,
        )
    }
}

impl Drop for GlesProgram {
    fn drop(&mut self) {
        let _ = self
            .destruction_callback_sender
            .send(CleanupResource::Program(self.program));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShaderVariant {
    Rgbx = 0,
    Rgba,
    External,
    Solid,
    Custom,
}

impl ShaderVariant {
    pub(super) fn from_texture(tex: &GlesTexture) -> Self {
        match tex.0.format.filter(|_| !tex.0.is_external) {
            Some(ffi::BGRA_EXT) | Some(ffi::RGBA) | Some(ffi::RGBA8) | Some(ffi::RGB10_A2)
            | Some(ffi::RGBA16F) => {
                if tex.0.has_alpha {
                    ShaderVariant::Rgba
                } else {
                    ShaderVariant::Rgbx
                }
            }
            None => ShaderVariant::External,
            _ => panic!("Unknown texture type"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurveType {
    Identity = 0,
    _3x1dLut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MappingType {
    Identity = 0,
    Matrix,
    _3dLut,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AttributeLocations {
    pub(super) vert: ffi::types::GLint,
    pub(super) vert_position: ffi::types::GLint,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct UniformLocations {
    pub(super) matrix: ffi::types::GLint,
    pub(super) tex_matrix: ffi::types::GLint,
    pub(super) alpha: ffi::types::GLint,
    pub(super) tex: ffi::types::GLint,
    pub(super) color: ffi::types::GLint,
    pub(super) tint: ffi::types::GLint,
    pub(super) color_pre_curve_lut_2d: ffi::types::GLint,
    pub(super) color_pre_curve_lut_scale_offset: ffi::types::GLint,
    pub(super) color_mapping_lut_3d: ffi::types::GLint,
    pub(super) color_mapping_lut_scale_offset: ffi::types::GLint,
    pub(super) color_mapping_matrix: ffi::types::GLint,
    pub(super) color_post_curve_lut_2d: ffi::types::GLint,
    pub(super) color_post_curve_lut_scale_offset: ffi::types::GLint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShaderSettings {
    pub variant: ShaderVariant,
    pub pre_curve: CurveType,
    pub mapping: MappingType,
    pub post_curve: CurveType,
    pub debug: bool,
}

impl ShaderSettings {
    fn add_settings_to_src(&self, src: impl AsRef<str>) -> String {
        let mut defines = String::new();
        defines.push_str(&format!("#define VARIANT {}\n", self.variant as i32));
        defines.push_str(&format!("#define PRE_CURVE {}\n", self.pre_curve as i32));
        defines.push_str(&format!("#define MAPPING {}\n", self.mapping as i32));
        defines.push_str(&format!("#define POST_CURVE {}\n", self.post_curve as i32));
        if self.debug {
            defines.push_str("#define DEBUG 1\n");
        }

        src.as_ref().replace("//_DEFINES_", &defines)
    }
}

pub(super) unsafe fn compile_shader(
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

pub(super) unsafe fn link_program(
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
