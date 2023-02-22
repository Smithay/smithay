/*
 * OpenGL Shaders
 */
pub const VERTEX_SHADER: &str = r#"
#version 100
uniform mat3 matrix;
uniform mat3 tex_matrix;

attribute vec2 vert;
attribute vec4 vert_position;

varying vec2 v_coords;

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

pub const XBGR: &str = "XBGR";
pub const EXTERNAL: &str = "EXTERNAL";
pub const DEBUG_FLAGS: &str = "DEBUG_FLAGS";

pub const FRAGMENT_SHADER: &str = r#"
#version 100
#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

void main() {
    vec4 color;

#if defined(XBGR)
    color = vec4(texture2D(tex, v_coords).rgb, 1.0) * alpha;
#else
    color = texture2D(tex, v_coords) * alpha;
#endif

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.3, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
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
    vec3 position = vec3(vert * scale(transform_scale) + transform_translation, 1.0);
    gl_Position = vec4(matrix * position, 1.0);
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
