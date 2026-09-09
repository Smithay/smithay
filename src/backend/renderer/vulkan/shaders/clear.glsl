#version 450

layout(
local_size_x = 8,
local_size_y = 8,
local_size_z = 1
) in;

layout(binding = 0, rgba8) uniform image2D dst;
layout(push_constant, std140) uniform PushConstants {
    vec4 color;
    bool blend;

    uint rectSize;
    ivec4 rects[6];
} params;

void main() {
    uvec2 coord = uvec2(gl_GlobalInvocationID.x, gl_GlobalInvocationID.y);
    uvec2 outSize = imageSize(dst);

    if (coord.x >= outSize.x || coord.y >= outSize.y)
        return;

    for (int i = 0; i < params.rectSize; i++) {
        ivec4 rect = params.rects[i];

        if (coord.x >= rect.x && coord.x < (rect.x + rect.z)
                && coord.y >= rect.y && coord.y < (rect.y + rect.w))
        {
            vec4 color = params.color;
            if (params.blend && color.a < 1.0) {
                vec4 blend = imageLoad(dst, ivec2(coord)).bgra - vec4(color.a);
                color = color + blend;
            }
            imageStore(dst, ivec2(coord), color.bgra);
            break;
        }
    }
}
