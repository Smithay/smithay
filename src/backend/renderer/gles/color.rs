use libc::c_void;

use super::*;
use crate::backend::color::{Curve, MappingLUT, Transformation, _1D_POINTS, _3D_POINTS};

type ShaderCache = RefCell<HashMap<usize, HashMap<(SourceHash, ShaderSettings), Arc<GlesProgram>>>>;

pub(super) struct GlCurveData {
    tex: ffi::types::GLuint,
    scale: f32,
    offset: f32,
}

pub(super) enum GlMappingData {
    Lut {
        tex: ffi::types::GLuint,
        scale: f32,
        offset: f32,
    },
    Matrix([f32; 9]),
}

struct GlTransformData {
    pre_curve: Option<GlCurveData>,
    mapping: Option<GlMappingData>,
    post_curve: Option<GlCurveData>,
    destruction_callback_sender: Sender<CleanupResource>,
}

impl Drop for GlTransformData {
    fn drop(&mut self) {
        if let Some(pre_curve) = self.pre_curve.take() {
            self.destruction_callback_sender
                .send(CleanupResource::Texture(pre_curve.tex));
        }
        if let Some(post_curve) = self.post_curve.take() {
            self.destruction_callback_sender
                .send(CleanupResource::Texture(post_curve.tex));
        }
        if let Some(GlMappingData::Lut { tex, .. }) = self.mapping.take() {
            self.destruction_callback_sender
                .send(CleanupResource::Texture(tex));
        }
    }
}

pub struct GlTransformState {
    pub(super) program: Arc<GlesProgram>,
    data: GlTransformData,
}

pub(super) fn gl_curve_lut_3x1d<C: Curve>(gl: &ffi::Gles2, curves: &[C; 3]) -> GlCurveData {
    const lut_len: usize = _1D_POINTS;
    let mut lut = [0.0; lut_len * 4]; // four rows, so that y-coords are centered in GLSL
    curves[0].fill_in(&mut lut[0..lut_len]);
    curves[1].fill_in(&mut lut[lut_len..(2 * lut_len)]);
    curves[2].fill_in(&mut lut[(2 * lut_len)..(3 * lut_len)]);

    let mut tex = 0;
    unsafe {
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.GenTextures(1, &mut tex);
        gl.BindTexture(ffi::TEXTURE_2D, tex);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
        gl.PixelStorei(ffi::UNPACK_ALIGNMENT, std::mem::size_of::<f32>() as i32);
        gl.PixelStorei(ffi::UNPACK_ROW_LENGTH_EXT, 0);
        gl.PixelStorei(ffi::UNPACK_SKIP_PIXELS_EXT, 0);
        gl.PixelStorei(ffi::UNPACK_SKIP_ROWS_EXT, 0);
        gl.TexImage2D(
            ffi::TEXTURE_2D,
            0,
            ffi::R32F as i32,
            lut_len as i32,
            4,
            0,
            ffi::RED,
            ffi::FLOAT,
            &lut as *const _ as *const c_void,
        );

        gl.BindTexture(ffi::TEXTURE_2D, 0);
    }

    GlCurveData {
        tex,
        scale: (lut_len as f32 - 1.0) / lut_len as f32,
        offset: 0.5 / lut_len as f32,
    }
}

pub(super) fn gl_mapping_lut_3d<M: MappingLUT>(gl: &ffi::Gles2, mapping: &M) -> GlMappingData {
    const dimension_len: usize = _3D_POINTS;
    let mut lut = [0.0; 3 * dimension_len * dimension_len * dimension_len];

    mapping.fill_in(&mut lut, _3D_POINTS);

    let mut tex = 0;
    unsafe {
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.GenTextures(1, &mut tex);
        gl.BindTexture(ffi::TEXTURE_3D, tex);
        gl.TexParameteri(ffi::TEXTURE_3D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
        gl.TexParameteri(ffi::TEXTURE_3D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
        gl.TexParameteri(ffi::TEXTURE_3D, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
        gl.TexParameteri(ffi::TEXTURE_3D, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
        gl.TexParameteri(ffi::TEXTURE_3D, ffi::TEXTURE_WRAP_R, ffi::CLAMP_TO_EDGE as i32);
        gl.PixelStorei(ffi::UNPACK_ROW_LENGTH_EXT, 0);
        gl.PixelStorei(ffi::UNPACK_SKIP_PIXELS_EXT, 0);
        gl.PixelStorei(ffi::UNPACK_SKIP_ROWS_EXT, 0);
        gl.TexImage3D(
            ffi::TEXTURE_3D,
            0,
            ffi::RGB32F as i32,
            dimension_len as i32,
            dimension_len as i32,
            dimension_len as i32,
            0,
            ffi::RGB,
            ffi::FLOAT,
            &lut as *const _ as *const c_void,
        );

        gl.BindTexture(ffi::TEXTURE_3D, 0);
    }

    GlMappingData::Lut {
        tex,
        scale: (dimension_len as f32 - 1.0) / dimension_len as f32,
        offset: 0.5 / dimension_len as f32,
    }
}

pub fn gl_state_from_transform<C: Transformation>(
    renderer: &mut GlesRenderer,
    variant: ShaderVariant,
    factory: Option<&mut ShaderFactory>,
    transform: &C,
) -> Result<GlTransformState, GlesError> {
    let pre_curve = transform.pre_curve();
    let mapping = transform.mapping();
    let post_curve = transform.post_curve();
    let settings = ShaderSettings {
        debug: !renderer.debug_flags.is_empty(),
        variant,
        pre_curve: if pre_curve.is_some() {
            CurveType::_3x1dLut
        } else {
            CurveType::Identity
        },
        mapping: match mapping.as_ref() {
            Some(Mapping::Matrix(_)) => MappingType::Matrix,
            Some(Mapping::LUT(_)) => MappingType::_3dLut,
            None => MappingType::Identity,
        },
        post_curve: if post_curve.is_some() {
            CurveType::_3x1dLut
        } else {
            CurveType::Identity
        },
    };

    let user_data = transform.user_data();
    user_data.insert_if_missing(|| ShaderCache::new(HashMap::new()));
    let transform_shaders = user_data.get::<ShaderCache>().unwrap();
    let mut transform_shaders_ref = transform_shaders.borrow_mut();

    // TODO check, that provided factory's renderer_id matches the passed in renderer.
    let factory = factory.unwrap_or(&mut renderer.buildin_shader);
    let our_shaders = transform_shaders_ref.entry(factory.renderer_id).or_default();
    let source_hash = *factory.source_hash();

    let program = match our_shaders.get(&(source_hash, settings)) {
        Some(shader) => shader.clone(),
        None => {
            let shader = factory.program_for_settings(&renderer.gl, &settings)?;
            our_shaders.insert((source_hash, settings), shader.clone());
            shader
        }
    };

    let data = GlTransformData {
        pre_curve: pre_curve.map(|curves| gl_curve_lut_3x1d(&renderer.gl, curves)),
        mapping: mapping.map(|mapping| match mapping {
            Mapping::LUT(lut) => gl_mapping_lut_3d(&renderer.gl, lut),
            Mapping::Matrix(matrix) => GlMappingData::Matrix(*matrix.as_ref()),
        }),
        post_curve: post_curve.map(|curves| gl_curve_lut_3x1d(&renderer.gl, curves)),
        destruction_callback_sender: renderer.destruction_callback_sender.clone(),
    };

    Ok(GlTransformState { program, data })
}

impl GlTransformState {
    pub unsafe fn set_uniforms<C: Transformation>(&self, renderer: &GlesRenderer) {
        unsafe {
            if let Some(gl_curve_lut) = self.data.pre_curve.as_ref() {
                renderer.gl.ActiveTexture(ffi::TEXTURE1);
                renderer.gl.BindTexture(ffi::TEXTURE_2D, gl_curve_lut.tex);
                renderer
                    .gl
                    .Uniform1i(self.program.uniform_locations.color_pre_curve_lut_2d, 1);
                renderer.gl.Uniform2f(
                    self.program.uniform_locations.color_pre_curve_lut_scale_offset,
                    gl_curve_lut.scale,
                    gl_curve_lut.offset,
                );
            }

            match &self.data.mapping {
                Some(GlMappingData::Matrix(mat)) => renderer.gl.UniformMatrix3fv(
                    self.program.uniform_locations.color_mapping_matrix,
                    1,
                    ffi::FALSE,
                    mat.as_ptr(),
                ),
                Some(GlMappingData::Lut { tex, scale, offset }) => {
                    renderer.gl.ActiveTexture(ffi::TEXTURE2);
                    renderer.gl.BindTexture(ffi::TEXTURE_3D, *tex);
                    renderer
                        .gl
                        .Uniform1i(self.program.uniform_locations.color_mapping_lut_3d, 2);
                    renderer.gl.Uniform2f(
                        self.program.uniform_locations.color_mapping_lut_scale_offset,
                        *scale,
                        *offset,
                    );
                }
                None => {}
            }

            if let Some(gl_curve_lut) = self.data.post_curve.as_ref() {
                renderer.gl.ActiveTexture(ffi::TEXTURE3);
                renderer.gl.BindTexture(ffi::TEXTURE_2D, gl_curve_lut.tex);
                renderer
                    .gl
                    .Uniform1i(self.program.uniform_locations.color_post_curve_lut_2d, 3);
                renderer.gl.Uniform2f(
                    self.program.uniform_locations.color_post_curve_lut_scale_offset,
                    gl_curve_lut.scale,
                    gl_curve_lut.offset,
                );
            }
        }
    }

    pub unsafe fn clear_uniforms(&self, renderer: &GlesRenderer) {
        unsafe {
            renderer.gl.ActiveTexture(ffi::TEXTURE1);
            renderer.gl.BindTexture(ffi::TEXTURE_2D, 0);
            renderer.gl.ActiveTexture(ffi::TEXTURE2);
            renderer.gl.BindTexture(ffi::TEXTURE_3D, 0);
            renderer.gl.ActiveTexture(ffi::TEXTURE3);
            renderer.gl.BindTexture(ffi::TEXTURE_2D, 0);
        }
    }
}
