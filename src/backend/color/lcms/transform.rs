use super::{super::*, Error, LcmsColorProfile};
use lcms2;

use std::{fmt, sync::Arc};

#[derive(Clone)]
pub struct LcmsColorTransform(pub(super) Arc<LcmsColorTransformInternal>);
pub(super) struct LcmsColorTransformInternal {
    pub(super) pre_curve: Option<[lcms2::ToneCurve; 3]>,
    pub(super) post_curve: Option<[lcms2::ToneCurve; 3]>,
    pub(super) mapping: Option<Mapping<LcmsMappingLUT>>,
    user_data: UserDataMap,
}

impl fmt::Debug for LcmsColorTransform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl fmt::Debug for LcmsColorTransformInternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LcmsColorTransform")
            .field("pre_curve", &self.pre_curve.is_some())
            .field("post_curve", &self.post_curve.is_some())
            .field("mapping", &self.mapping)
            .field("user_data", &self.user_data)
            .finish()
    }
}

pub(super) fn realize_chain(
    ctx: &lcms2::ThreadContext,
    input: &LcmsColorProfile,
    output: &LcmsColorProfile,
    type_: TransformType,
) -> Result<LcmsColorTransformInternal, Error> {
    // alright, lets build a custom transformation chain

    let mut chain = Vec::with_capacity(3);
    // 1. input-profile
    chain.push(&input.0.profile);
    // 2. output-profile
    chain.push(&output.0.profile);

    let linearization_profile = match type_ {
        TransformType::InputToBlend => {
            // 3. Linearize to make blending well-defined
            let curves = output
                .0
                .output_curves(ctx)
                .ok_or(Error::BadOutputProfile)?
                .eotf
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<_>>();
            // SAFETY: We only deal with RGB contexts, so the number of three curves matches up
            Some(unsafe {
                lcms2::Profile::new_linearization_device_link_context(
                    &ctx,
                    lcms2::ColorSpaceSignature::RgbData,
                    &curves,
                )?
            })
        }
        TransformType::InputToOutput => {
            // 3. Add VCGT for output
            if let Some(vcgt) = output
                .0
                .output_curves(ctx)
                .ok_or(Error::BadOutputProfile)?
                .vcgt
                .as_ref()
            {
                let curves = vcgt.iter().map(AsRef::as_ref).collect::<Vec<_>>();
                // SAFETY: We only deal with RGB contexts, so the number of three curves matches up
                Some(unsafe {
                    lcms2::Profile::new_linearization_device_link_context(
                        &ctx,
                        lcms2::ColorSpaceSignature::RgbData,
                        &curves,
                    )?
                })
            } else {
                None
            }
        }
        TransformType::BlendToOutput => {
            // Handled by caller
            None
        }
    };
    if let Some(profile) = linearization_profile.as_ref() {
        chain.push(&profile);
    }

    let lut = lcms2::Transform::new_multiprofile_context(
        ctx,
        &chain,
        lcms2::PixelFormat::RGB_FLT,
        lcms2::PixelFormat::RGB_FLT,
        // **TODO**: Take into account client provided content profile,
        // output profile, and the category of the wanted color transformation.
        lcms2::Intent::RelativeColorimetric,
        lcms2::Flags::default(),
    )?;

    Ok(LcmsColorTransformInternal {
        pre_curve: None,
        mapping: Some(Mapping::LUT(LcmsMappingLUT(lut))),
        post_curve: None,
        user_data: UserDataMap::new(),
    })
}

impl super::Transformation for LcmsColorTransform {
    type Curve = lcms2::ToneCurve;
    type MappingLUT = LcmsMappingLUT;

    fn pre_curve(&self) -> Option<&[Self::Curve; 3]> {
        self.0.pre_curve.as_ref()
    }
    fn mapping(&self) -> Option<&Mapping<Self::MappingLUT>> {
        self.0.mapping.as_ref()
    }
    fn post_curve(&self) -> Option<&[Self::Curve; 3]> {
        self.0.post_curve.as_ref()
    }

    fn user_data(&self) -> &UserDataMap {
        &self.0.user_data
    }
}

#[derive(Debug)]
pub struct LcmsMappingLUT(lcms2::Transform<[f32; 3], [f32; 3], lcms2::ThreadContext, lcms2::AllowCache>);

impl super::MappingLUT for LcmsMappingLUT {
    fn fill_in(&self, lut: &mut [f32], len: usize) {
        assert_eq!(lut.len() % 3, 0);
        let divider = (len - 1) as f32;

        let mut rgb_in = [[0.0, 0.0, 0.0]];
        let mut rgb_out = [[0.0, 0.0, 0.0]];

        for value_b in 0..len {
            for value_g in 0..len {
                for value_r in 0..len {
                    rgb_in[0] = [
                        (value_r as f32) / divider,
                        (value_g as f32) / divider,
                        (value_b as f32) / divider,
                    ];

                    self.0.transform_pixels(&rgb_in, &mut rgb_out);

                    let idx = 3 * (value_r + len * (value_g + len * value_b));
                    lut[idx] = rgb_out[0][0].clamp(0.0, 1.0);
                    lut[idx + 1] = rgb_out[0][1].clamp(0.0, 1.0);
                    lut[idx + 2] = rgb_out[0][2].clamp(0.0, 1.0);
                }
            }
        }
    }
}
