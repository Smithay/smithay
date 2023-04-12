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