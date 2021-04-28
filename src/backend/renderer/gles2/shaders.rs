/*
 * OpenGL Shaders
 */
pub const VERTEX_SHADER: &str = r#"
#version 100
uniform mat3 matrix;
uniform bool invert_y;
attribute vec2 position;
attribute vec2 tex_coords;
varying vec2 v_tex_coords;
void main() {
    gl_Position = vec4(matrix * vec3(position, 1.0), 1.0);
    if (invert_y) {
        v_tex_coords = vec2(tex_coords.x, 1.0 - tex_coords.y);
    } else {
        v_tex_coords = tex_coords;
    }
}"#;

pub const FRAGMENT_COUNT: usize = 5;

pub const FRAGMENT_SHADER_ABGR: &str = r#"
#version 100
precision mediump float;
uniform sampler2D tex;
uniform float alpha;
varying vec2 v_tex_coords;
void main() {
    vec4 color = texture2D(tex, v_tex_coords);
    gl_FragColor.r = color.w;
    gl_FragColor.g = color.z;
    gl_FragColor.b = color.y;
    gl_FragColor.a = color.x * alpha;
}
"#;

pub const FRAGMENT_SHADER_XBGR: &str = r#"
#version 100
precision mediump float;
uniform sampler2D tex;
uniform float alpha;
varying vec2 v_tex_coords;
void main() {
    vec4 color = texture2D(tex, v_tex_coords);
    gl_FragColor.r = color.w;
    gl_FragColor.g = color.z;
    gl_FragColor.b = color.y;
    gl_FragColor.a = alpha;
}
"#;

pub const FRAGMENT_SHADER_BGRA: &str = r#"
#version 100
precision mediump float;
uniform sampler2D tex;
uniform float alpha;
varying vec2 v_tex_coords;
void main() {
    vec4 color = texture2D(tex, v_tex_coords);
    gl_FragColor.r = color.z;
    gl_FragColor.g = color.y;
    gl_FragColor.b = color.x;
    gl_FragColor.a = color.w * alpha;
}
"#;

pub const FRAGMENT_SHADER_BGRX: &str = r#"
#version 100
precision mediump float;
uniform sampler2D tex;
uniform float alpha;
varying vec2 v_tex_coords;
void main() {
    vec4 color = texture2D(tex, v_tex_coords);
    gl_FragColor.r = color.z;
    gl_FragColor.g = color.y;
    gl_FragColor.b = color.x;
    gl_FragColor.a = alpha;
}
"#;

pub const FRAGMENT_SHADER_EXTERNAL: &str = r#"
#version 100
#extension GL_OES_EGL_image_external : require
precision mediump float;
uniform samplerExternalOES tex;
uniform float alpha;
varying vec2 v_tex_coords;
void main() {
    gl_FragColor = texture2D(tex, v_tex_coords) * alpha;
}
"#;
