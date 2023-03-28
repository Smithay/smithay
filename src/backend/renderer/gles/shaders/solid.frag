#version 100

precision mediump float;
uniform vec4 color;

void main() {
    gl_FragColor = color;
}