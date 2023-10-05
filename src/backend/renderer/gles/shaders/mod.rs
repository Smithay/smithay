mod color;
mod implicit;

use std::fmt::Write;

pub(super) use color::*;
pub use implicit::*;

// define constants
/// No alpha shader define
pub const NO_ALPHA: &str = "NO_ALPHA";
/// External texture shader define
pub const EXTERNAL: &str = "EXTERNAL";
/// Debug flags shader define
pub const DEBUG_FLAGS: &str = "DEBUG_FLAGS";

use super::*;

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

pub(super) unsafe fn texture_program(
    gl: &ffi::Gles2,
    src: &str,
    additional_uniforms: &[UniformName<'_>],
    destruction_callback_sender: Sender<CleanupResource>,
) -> Result<GlesTexProgram, GlesError> {
    let create_variant = |defines: &[&str]| -> Result<GlesTexProgramVariant, GlesError> {
        let shader = src.replace(
            "//_DEFINES_",
            &defines.iter().fold(String::new(), |mut shader, define| {
                let _ = writeln!(&mut shader, "#define {}", define);
                shader
            }),
        );
        let debug_shader = src.replace(
            "//_DEFINES_",
            &defines
                .iter()
                .chain(&[shaders::DEBUG_FLAGS])
                .fold(String::new(), |mut shader, define| {
                    let _ = writeln!(shader, "#define {}", define);
                    shader
                }),
        );

        let program = unsafe { link_program(gl, shaders::VERTEX_SHADER, &shader)? };
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

pub(super) unsafe fn solid_program(gl: &ffi::Gles2) -> Result<GlesSolidProgram, GlesError> {
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

pub(super) unsafe fn color_output_program(
    gl: &ffi::Gles2,
    destruction_callback_sender: Sender<CleanupResource>,
) -> Result<GlesColorOutputProgram, GlesError> {
    let program = link_program(gl, shaders::OUTPUT_VERTEX_SHADER, shaders::OUTPUT_FRAGMENT_SHADER)?;

    let vert = CStr::from_bytes_with_nul(b"vert\0").expect("NULL terminated");
    let tex = CStr::from_bytes_with_nul(b"tex\0").expect("NULL terminated");
    Ok(GlesColorOutputProgram {
        program,
        attrib_vert: gl.GetAttribLocation(program, vert.as_ptr() as *const ffi::types::GLchar),
        uniform_tex: gl.GetUniformLocation(program, tex.as_ptr() as *const ffi::types::GLchar),
        destruction_callback_sender,
    })
}
