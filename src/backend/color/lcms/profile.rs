use lcms2;
use once_cell::unsync::OnceCell;
use tracing::trace;

use std::{
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};

#[derive(Clone)]
pub struct LcmsColorProfile(pub(super) Arc<LcmsColorProfileInternal>);
pub(super) struct LcmsColorProfileInternal {
    pub(super) profile: lcms2::Profile<lcms2::ThreadContext>,
    output_curves: OnceCell<Option<OutputCurves>>,
}

impl fmt::Debug for LcmsColorProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl fmt::Debug for LcmsColorProfileInternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LcmsColorProfile")
            .field("profile", &"...")
            .field("output_curves", &self.output_curves)
            .finish()
    }
}

fn read_tone_curve(
    profile: &lcms2::Profile<lcms2::ThreadContext>,
    sig: lcms2::TagSignature,
) -> Option<lcms2::ToneCurve> {
    let lcms2::Tag::ToneCurve(curve) = profile.read_tag(sig) else {
        trace!(?sig, "Matrix shaper profile does not contain mandatory tag.");
        return None;
    };

    Some(curve.to_owned())
}

#[derive(Debug, Default, Clone, Copy)]
#[repr(transparent)]
struct XYZArrF32([f32; 3]);
impl XYZArrF32 {
    fn dot_product(&self, other: &Self) -> f64 {
        self.0[0] as f64 * other.0[0] as f64
            + self.0[1] as f64 * other.0[1] as f64
            + self.0[2] as f64 * other.0[2] as f64
    }
}

fn build_tone_curve(
    transform: &lcms2::Transform<[f32; 3], XYZArrF32, lcms2::ThreadContext>,
    idx: usize,
) -> Option<lcms2::ToneCurve> {
    const NUM_POINTS: usize = super::_1D_POINTS;
    let divider = (NUM_POINTS - 1) as f32;

    let mut curve = [0.0f32; NUM_POINTS];
    let mut rgb = [[0.0, 0.0, 0.0]];
    rgb[0][idx] = 1.0;

    let mut prim_xyz_max = [XYZArrF32::default()];
    transform.transform_pixels(&rgb, &mut prim_xyz_max);

    let xyz_square_magnitude = prim_xyz_max[0].dot_product(&prim_xyz_max[0]);

    let mut prim_xyz = [XYZArrF32::default()];
    for i in 0..NUM_POINTS {
        rgb[0][idx] = i as f32 / divider;
        transform.transform_pixels(&rgb, &mut prim_xyz);
        curve[i] = (prim_xyz[0].dot_product(&prim_xyz_max[0]) / xyz_square_magnitude) as f32;
    }

    let tone_curve = lcms2::ToneCurve::new_tabulated_float(&curve);
    if !tone_curve.is_monotonic() {
        trace!(channel = idx, "Resulting tone curve is not monotonic");
        None
    } else {
        Some(tone_curve)
    }
}

fn concat_monotonic_tone_curves(x: &lcms2::ToneCurveRef, y: &lcms2::ToneCurveRef) -> lcms2::ToneCurve {
    const NUM_POINTS: usize = super::_1D_POINTS;
    let divider = (NUM_POINTS - 1) as f32;
    let mut curve = [0.0f32; NUM_POINTS];

    for i in 0..NUM_POINTS {
        let value = i as f32 / divider;
        curve[i] = y.eval(x.eval(value));
    }

    lcms2::ToneCurve::new_tabulated_float(&curve)
}

impl LcmsColorProfileInternal {
    pub fn new(profile: lcms2::Profile<lcms2::ThreadContext>) -> Arc<LcmsColorProfileInternal> {
        Arc::new(LcmsColorProfileInternal {
            profile,
            output_curves: OnceCell::new(),
        })
    }

    pub(super) fn output_curves(&self, ctx: &lcms2::ThreadContext) -> Option<&OutputCurves> {
        self.output_curves
            .get_or_init(|| {
                let eotf = if self.profile.is_matrix_shaper() {
                    // Matrix shaper profiles may have
                    // - 1D-LUT -> 3x3 -> 3x3 -> 1D-LUT
                    // - 1D-LUT -> 3x3 -> 1D-LUT

                    [
                        read_tone_curve(&self.profile, lcms2::TagSignature::RedTRCTag)?,
                        read_tone_curve(&self.profile, lcms2::TagSignature::GreenTRCTag)?,
                        read_tone_curve(&self.profile, lcms2::TagSignature::BlueTRCTag)?,
                    ]
                } else {
                    // Linearization of cLUT profiles may have
                    // - 1D-LUT -> 3D-LUT -> 1D-LUT
                    // - 1D-LUT -> 3D-LUT
                    // - 3D-LUT

                    let xyz_profile = lcms2::Profile::new_xyz_context(ctx);
                    let transform_rgb_to_xyz = match lcms2::Transform::new_context(
                        ctx,
                        &self.profile,
                        lcms2::PixelFormat::RGB_FLT,
                        &xyz_profile,
                        lcms2::PixelFormat::XYZ_FLT,
                        lcms2::Intent::AbsoluteColorimetric,
                    ) {
                        Ok(transform) => transform,
                        Err(err) => {
                            trace!(?err, "Profile can't be transformed into XYZ space");
                            return None;
                        }
                    };

                    [
                        build_tone_curve(&transform_rgb_to_xyz, 0)?,
                        build_tone_curve(&transform_rgb_to_xyz, 1)?,
                        build_tone_curve(&transform_rgb_to_xyz, 2)?,
                    ]
                };

                let mut eotf_inv = [eotf[0].reversed(), eotf[1].reversed(), eotf[2].reversed()];

                let vcgt = if let lcms2::Tag::VcgtCurves(curves) =
                    self.profile.read_tag(lcms2::TagSignature::VcgtTag)
                {
                    for (i, curve) in curves.iter().enumerate() {
                        eotf_inv[i] = concat_monotonic_tone_curves(eotf_inv[i].as_ref(), *curve);
                    }
                    Some(curves.map(lcms2::ToneCurveRef::to_owned))
                } else {
                    None
                };

                Some(OutputCurves {
                    eotf,
                    output_inv_eotf_vcgt: eotf_inv,
                    vcgt,
                })
            })
            .as_ref()
    }
}

pub(super) struct OutputCurves {
    pub(super) eotf: [lcms2::ToneCurve; 3],
    pub(super) output_inv_eotf_vcgt: [lcms2::ToneCurve; 3],
    pub(super) vcgt: Option<[lcms2::ToneCurve; 3]>,
}

impl fmt::Debug for OutputCurves {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutputCurves")
            .field("has_vcgt", &self.vcgt.is_some())
            .finish_non_exhaustive()
    }
}

impl AsRef<lcms2::Profile<lcms2::ThreadContext>> for LcmsColorProfile {
    fn as_ref(&self) -> &lcms2::Profile<lcms2::ThreadContext> {
        &self.0.profile
    }
}

impl Hash for LcmsColorProfile {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.profile.profile_id().hash(state)
    }
}

impl PartialEq for LcmsColorProfile {
    fn eq(&self, other: &Self) -> bool {
        self.0.profile.profile_id() == other.0.profile.profile_id()
    }
}
