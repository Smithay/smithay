/*
 * This file is the single point of definition of the opengl shaders
 * and their indexes.
 *
 * The opengl_programs!() macro must call make_program!() in the correct
 * order matching the indices stored in the BUFFER_* constants, if it
 * does not, things will be drawn on screen with wrong colors.
 */

// create a set of shaders for various loading types
macro_rules! make_program(
    ($display: expr, $fragment_shader:expr) => {
        program!($display,
            100 => {
                vertex: crate::shaders::VERTEX_SHADER,
                fragment: $fragment_shader,
            },
        ).unwrap()
    }
);

#[macro_escape]
macro_rules! opengl_programs(
    ($display: expr) => {
        [
            make_program!($display, crate::shaders::FRAGMENT_SHADER_RGBA),
            make_program!($display, crate::shaders::FRAGMENT_SHADER_ABGR),
            make_program!($display, crate::shaders::FRAGMENT_SHADER_XBGR),
            make_program!($display, crate::shaders::FRAGMENT_SHADER_BGRA),
            make_program!($display, crate::shaders::FRAGMENT_SHADER_BGRX),
        ]
    }
);

/*
 * OpenGL Shaders
 */

pub const VERTEX_SHADER: &str = r#"
#version 100
uniform lowp mat4 matrix;
attribute lowp vec2 position;
attribute lowp vec2 tex_coords;
varying lowp vec2 v_tex_coords;
void main() {
    gl_Position = matrix * vec4(position, 0.0, 1.0);
    v_tex_coords = tex_coords;
}"#;

pub const FRAGMENT_COUNT: usize = 5;

pub const BUFFER_RGBA: usize = 0;
pub const FRAGMENT_SHADER_RGBA: &str = r#"
#version 100
uniform lowp sampler2D tex;
varying lowp vec2 v_tex_coords;
void main() {
    lowp vec4 color = texture2D(tex, v_tex_coords);
    gl_FragColor.r = color.x;
    gl_FragColor.g = color.y;
    gl_FragColor.b = color.z;
    gl_FragColor.a = color.w;
}
"#;

pub const BUFFER_ABGR: usize = 1;
pub const FRAGMENT_SHADER_ABGR: &str = r#"
#version 100
uniform lowp sampler2D tex;
varying lowp vec2 v_tex_coords;
void main() {
    lowp vec4 color = texture2D(tex, v_tex_coords);
    gl_FragColor.r = color.w;
    gl_FragColor.g = color.z;
    gl_FragColor.b = color.y;
    gl_FragColor.a = color.x;
}
"#;

pub const BUFFER_XBGR: usize = 2;
pub const FRAGMENT_SHADER_XBGR: &str = r#"
#version 100
uniform lowp sampler2D tex;
varying lowp vec2 v_tex_coords;
void main() {
    lowp vec4 color = texture2D(tex, v_tex_coords);
    gl_FragColor.r = color.w;
    gl_FragColor.g = color.z;
    gl_FragColor.b = color.y;
    gl_FragColor.a = 1.0;
}
"#;

pub const BUFFER_BGRA: usize = 3;
pub const FRAGMENT_SHADER_BGRA: &str = r#"
#version 100
uniform lowp sampler2D tex;
varying lowp vec2 v_tex_coords;
void main() {
    lowp vec4 color = texture2D(tex, v_tex_coords);
    gl_FragColor.r = color.z;
    gl_FragColor.g = color.y;
    gl_FragColor.b = color.x;
    gl_FragColor.a = color.w;
}
"#;

pub const BUFFER_BGRX: usize = 4;
pub const FRAGMENT_SHADER_BGRX: &str = r#"
#version 100
uniform lowp sampler2D tex;
varying lowp vec2 v_tex_coords;
void main() {
    lowp vec4 color = texture2D(tex, v_tex_coords);
    gl_FragColor.r = color.z;
    gl_FragColor.g = color.y;
    gl_FragColor.b = color.x;
    gl_FragColor.a = 1.0;
}
"#;
