/*
 * OpenGL Shaders
 */
pub const VERTEX_SHADER: &str = r#"

#version 100
uniform mat3 matrix;
uniform bool invert_y;

attribute vec2 vert;
attribute vec2 tex_coords;
attribute vec4 position;

varying vec2 v_tex_coords;

mat2 scale(vec2 scale_vec){
    return mat2(
        scale_vec.x, 0.0,
        0.0, scale_vec.y
    );
}

void main() {
    if (invert_y) {
        v_tex_coords = vec2(tex_coords.x, 1.0 - tex_coords.y);
    } else {
        v_tex_coords = tex_coords;
    }
    
    vec2 transform_translation = position.xy;
    vec2 transform_scale = position.zw;
    v_tex_coords = (vec3((tex_coords * scale(transform_scale)) + transform_translation, 1.0)).xy;
    gl_Position = vec4(matrix * vec3((vert * scale(transform_scale)) + transform_translation, 1.0), 1.0);
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
