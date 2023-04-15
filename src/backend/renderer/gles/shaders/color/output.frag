#version 300 es

// TODO color transformation

precision highp float;

uniform sampler2D tex;
out vec4 color;

void main() {
    color = texelFetch(tex, ivec2(gl_FragCoord.xy), 0);
}