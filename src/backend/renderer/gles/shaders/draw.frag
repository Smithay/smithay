#version 100

//_DEFINES_

//compositor injected
// DEBUG
// VARIANT
// PRE_CURVE
// POST_CURVE
// MAPPING

#define VARIANT_RGBX  0
#define VARIANT_RGBA  1
#define VARIANT_EXTERNAL  2
#define VARIANT_SOLID 3

#define CURVE_IDENTITY 0
#define CURVE_LUT_3x1D 1

#define MAPPING_IDENTITY 0
#define MAPPING_MATRIX 1
#define MAPPING_3DLUT 2

#if VARIANT == VARIANT_EXTERNAL
#extension GL_OES_EGL_image_external : require
#endif

#if MAPPING == MAPPING_3DLUT
#extension GL_OES_texture_3D : require
#endif

#ifdef GL_FRAGMENT_PRECISION_HIGH
#define PRECISION highp
precision highp float;
#else
#define PRECISION mediump
precision mediump float;
#endif

#if VARIANT == VARIANT_EXTERNAL
uniform samplerExternalOES tex;
#else
#if VARIANT == VARIANT_SOLID
uniform vec3 color;
#else
uniform sampler2D tex;
#endif
#endif

uniform float alpha;
varying PRECISION vec2 v_coords;

#ifdef DEBUG
uniform float tint;
#endif

#if PRE_CURVE != CURVE_IDENTITY
uniform PRECISION sampler2D color_pre_curve_lut_2d;
uniform PRECISION vec2 color_pre_curve_lut_scale_offset;
#endif
#if POST_CURVE != CURVE_IDENTITY
uniform PRECISION sampler2D color_post_curve_lut_2d;
uniform PRECISION vec2 color_post_curve_lut_scale_offset;
#endif

#if MAPPING == MAPPING_3DLUT
uniform PRECISION sampler3D color_mapping_lut_3d;
uniform PRECISION vec2 color_mapping_lut_scale_offset;
#else
uniform PRECISION mat3 color_mapping_matrix;
#endif


vec4 input_color() {
#if VARIANT == VARIANT_SOLID
	return vec4(color, alpha);
#else

#if VARIANT == VARIANT_RGBX
	return vec4(texture2D(tex, v_coords).rgb, alpha);
#else
    vec4 color = texture2D(tex, v_coords);
    color.a *= alpha;
    return color;
#endif

#endif
}

#if PRE_CURVE != CURVE_IDENTITY || POST_CURVE != CURVE_IDENTITY
/*
 * Texture coordinates go from 0.0 to 1.0 corresponding to texture edges.
 * When we do LUT look-ups with linear filtering, the correct range to sample
 * from is not from edge to edge, but center of first texel to center of last
 * texel. This follows because with LUTs, you have the exact end points given,
 * you never extrapolate but only interpolate.
 * The scale and offset are precomputed to achieve this mapping.
 */
float lut_texcoord(float x, vec2 scale_offset)
{
	return x * scale_offset.s + scale_offset.t;
}

/*
 * Sample a 1D LUT which is a single row of a 2D texture. The 2D texture has
 * four rows so that the centers of texels have precise y-coordinates.
 */
float sample_color_curve_lut_2d(float x, sampler2D curve, vec2 scale_offset, const int row)
{
	float tx = lut_texcoord(x, scale_offset);

	return texture2D(curve, vec2(tx, (float(row) + 0.5) / 4.0)).x;
}

vec3 apply_curve(vec3 color, sampler2D curve, vec2 scale_offset)
{
    return vec3(
        sample_color_curve_lut_2d(color.r, curve, scale_offset, 0),
        sample_color_curve_lut_2d(color.g, curve, scale_offset, 1),
        sample_color_curve_lut_2d(color.b, curve, scale_offset, 2)
    );
}
#endif

#if MAPPING == MAPPING_3DLUT
vec3 lut_texcoord(vec3 pos, vec2 scale_offset)
{
	return pos * scale_offset.s + scale_offset.t;
}

vec3 sample_color_mapping_lut_3d(vec3 color)
{
	vec3 pos = lut_texcoord(color, color_mapping_lut_scale_offset);
	return texture3D(color_mapping_lut_3d, pos).rgb;
}
#endif

#if MAPPING != MAPPING_IDENTITY
vec3 apply_mapping(vec3 color)
{
#if MAPPING == MAPPING_3DLUT
    return sample_color_mapping_lut_3d(color);
#else
	return color_mapping_matrix * color;
#endif
}
#endif


void main() {
    vec4 color = input_color();

    color.rgb *= 1.0 / color.a;

#if PRE_CURVE != CURVE_IDENTITY
    color.rgb = apply_curve(color.rgb, color_pre_curve_lut_2d, color_pre_curve_lut_scale_offset);
#endif
#if MAPPING != MAPPING_IDENTITY
    color.rgb = apply_mapping(color.rgb);
#endif
#if POST_CURVE != CURVE_IDENTITY
    color.rgb = apply_curve(color.rgb, color_post_curve_lut_2d, color_post_curve_lut_scale_offset);
#endif
    
    color.rgb *= color.a;

#ifdef DEBUG
    if (tint == 1.0)
        color = vec4(0.0, 0.3, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}