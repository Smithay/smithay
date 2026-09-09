#version 450

layout(
local_size_x = 8,
local_size_y = 8,
local_size_z = 1
) in;

layout(binding = 0, rgba8) uniform image2D dst;
layout(binding = 1) uniform sampler2D tex;
layout(push_constant, std140) uniform PushConstants {
    vec4 srcRect;
    vec4 dstRect;
    uint srcTransform;
    float alpha;

    uint damageSize;
    ivec4 damage[4];
} params;

vec2 applyTransform(vec2 uv, uint transform) {
    switch (transform) {
        case 0:
        return uv;
        case 1: // 90
        return vec2(1.0-uv.y, uv.x);
        case 2: // 180
        return vec2(1.0-uv.x, 1.0-uv.y);
        case 3: // 270
        return vec2(uv.y, 1.0-uv.x);
        case 4: // Flipped
        return vec2(1.0-uv.x, uv.y);
        case 5: // Flipped 90
        return vec2(1.0-uv.y, 1.0-uv.x);
        case 6: // Flipped 180
        return vec2(uv.x, 1.0-uv.y);
        case 7: // Flipped 270
        return vec2(uv.y, uv.x);
    }
}

void main() {
    uvec2 coord = uvec2(gl_GlobalInvocationID.x, gl_GlobalInvocationID.y);

    if (coord.x < params.dstRect.x || coord.x >= (params.dstRect.x + params.dstRect.z) || coord.y < params.dstRect.y || coord.y >= (params.dstRect.y + params.dstRect.w))
        return;

    uvec2 outSize = imageSize(dst);
    uvec2 texSize = textureSize(tex, 0);
    vec2 dstUV = (vec2(coord) - params.dstRect.xy) / params.dstRect.zw;
    vec2 srcUV = ((dstUV * params.srcRect.zw) + params.srcRect.xy) / texSize;
    vec2 uv = applyTransform(srcUV, params.srcTransform);

    for (int i = 0; i < params.damageSize; i++) {
        ivec4 rect = params.damage[i];

        if (coord.x >= rect.x && coord.x < (rect.x + rect.z) &&
            coord.y >= rect.y && coord.y < (rect.y + rect.w))
        {
            vec4 color = texture(tex, uv) * params.alpha;
            if (color.a < 1.0) {
                vec4 blend = imageLoad(dst, ivec2(coord)).bgra - vec4(color.a);
                color = color + blend;
            }
            imageStore(dst, ivec2(coord), color.bgra);
            break;
        }
    }
}
