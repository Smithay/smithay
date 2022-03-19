/*
 * OpenGL Shaders
 */
pub const VERTEX_SHADER: &str = r#"
#version 100
uniform mat3 matrix;
uniform mat3 tex_matrix;

attribute vec2 vert;
attribute vec4 vert_position;

varying vec2 v_tex_coords;

mat2 scale(vec2 scale_vec){
    return mat2(
        scale_vec.x, 0.0,
        0.0, scale_vec.y
    );
}

void main() {
    vec2 vert_transform_translation = vert_position.xy;
    vec2 vert_transform_scale = vert_position.zw;
    vec3 position = vec3(vert * scale(vert_transform_scale) + vert_transform_translation, 1.0);
    v_tex_coords = (tex_matrix * position).xy;
    gl_Position = vec4(matrix * position, 1.0);
}
"#;

pub const FRAGMENT_COUNT: usize = 3;

pub const FRAGMENT_SHADER_ABGR: &str = r#"
#version 100

precision mediump float;
uniform sampler2D tex;
uniform float alpha;
varying vec2 v_tex_coords;

void main() {
    gl_FragColor = texture2D(tex, v_tex_coords) * alpha;
}
"#;

pub const FRAGMENT_SHADER_XBGR: &str = r#"
#version 100

precision mediump float;
uniform sampler2D tex;
uniform float alpha;
varying vec2 v_tex_coords;

void main() {
    gl_FragColor = vec4(texture2D(tex, v_tex_coords).rgb, 1.0) * alpha;
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

pub const VERTEX_SHADER_SOLID: &str = r#"
#version 100

uniform mat3 matrix;
attribute vec2 vert;
attribute vec4 position;

mat2 scale(vec2 scale_vec){
    return mat2(
        scale_vec.x, 0.0,
        0.0, scale_vec.y
    );
}

void main() {
    vec2 transform_translation = position.xy;
    vec2 transform_scale = position.zw;
    gl_Position = vec4(matrix * vec3((vert * scale(transform_scale)) + transform_translation, 1.0), 1.0);
}
"#;

pub const FRAGMENT_SHADER_SOLID: &str = r#"
#version 100

precision mediump float;
uniform vec4 color;

void main() {
    gl_FragColor = color;
}
"#;
