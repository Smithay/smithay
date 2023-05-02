pub use lcms2;
use lcms2::ColorSpaceSignatureExt;
use once_cell::unsync::OnceCell;
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

mod profile;
pub use self::profile::*;

mod transform;
pub use self::transform::*;

use super::*;

#[derive(Debug)]
pub struct LcmsContext {
    ctx: lcms2::ThreadContext,
    profile_srgb: OnceCell<LcmsColorProfile>,
    profile_cache: HashMap<lcms2_sys::ffi::ProfileID, Weak<LcmsColorProfileInternal>>,
    transform_cache: HashMap<SearchParams, Weak<LcmsColorTransformInternal>>,
}

impl LcmsContext {
    pub fn new() -> LcmsContext {
        LcmsContext {
            ctx: lcms2::ThreadContext::new(),
            profile_srgb: OnceCell::new(),
            profile_cache: HashMap::new(),
            transform_cache: HashMap::new(),
        }
    }

    pub fn profile_from_rgb(
        &mut self,
        white: lcms2::CIExyY,
        red: lcms2::CIExyY,
        green: lcms2::CIExyY,
        blue: lcms2::CIExyY,
        tf: &[&lcms2::ToneCurve],
    ) -> Result<LcmsColorProfile, lcms2::Error> {
        let mut profile = lcms2::Profile::new_rgb_context(
            &self.ctx,
            &white,
            &lcms2::CIExyYTRIPLE {
                Red: red,
                Green: green,
                Blue: blue,
            },
            tf,
        )?;

        profile.set_default_profile_id();
        let id = profile.profile_id();

        let profile = LcmsColorProfileInternal::new(profile);

        self.profile_cache.retain(|_, value| value.upgrade().is_some());
        let profile_ref = self
            .profile_cache
            .entry(id)
            .or_insert_with(|| Arc::downgrade(&profile));

        Ok(LcmsColorProfile(profile_ref.upgrade().unwrap()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ICC profile is required to be of Display device class")]
    BadDeviceClass,
    #[error("ICC color profile must contain three channels for the color space")]
    BadColorFormat,
    #[error("ICC major version is unsupported, must be 2 or 4")]
    BadVersion,
    #[error("Provided profile is no valid output profile")]
    BadOutputProfile,
    #[error(transparent)]
    Lcms2(#[from] lcms2::Error),
}

impl CMS for LcmsContext {
    type Error = Error;
    type ColorProfile = LcmsColorProfile;
    type ColorTransformation = LcmsColorTransform;

    fn profile_srgb(&self) -> Self::ColorProfile {
        self.profile_srgb
            .get_or_init(|| {
                LcmsColorProfile(LcmsColorProfileInternal::new({
                    let mut profile = lcms2::Profile::new_srgb_context(&self.ctx);
                    profile.set_default_profile_id();
                    profile
                }))
            })
            .clone()
    }
    fn profile_from_icc(&mut self, icc: &[u8]) -> Result<Self::ColorProfile, Self::Error> {
        let mut profile = lcms2::Profile::new_icc_context(&self.ctx, icc)?;

        // validate, that we can use the provided profile
        let major: u8 = (profile.encoded_icc_version() >> 24) as u8;
        if major != 2 || major != 4 {
            return Err(Error::BadVersion);
        }
        if profile.color_space().channels() != 3 {
            return Err(Error::BadColorFormat);
        }
        if profile.device_class() != lcms2::ProfileClassSignature::DisplayClass {
            return Err(Error::BadDeviceClass);
        }

        profile.set_default_profile_id();
        let id = profile.profile_id();

        let profile = LcmsColorProfileInternal::new(profile);

        self.profile_cache.retain(|_, value| value.upgrade().is_some());
        let profile_ref = self
            .profile_cache
            .entry(id)
            .or_insert_with(|| Arc::downgrade(&profile));

        Ok(LcmsColorProfile(profile_ref.upgrade().unwrap()))
    }

    fn input_transformation(
        &mut self,
        input: &Self::ColorProfile,
        output: &Self::ColorProfile,
        type_: TransformType,
    ) -> Result<Self::ColorTransformation, Self::Error> {
        let search_params = SearchParams {
            input: Some(input.0.profile.profile_id()),
            output: output.0.profile.profile_id(),
            type_: type_.into(),
        };

        self.transform_cache.retain(|_, value| value.upgrade().is_some());
        let transform = if let Some(weak_transform) = self.transform_cache.get(&search_params) {
            weak_transform.upgrade().unwrap()
        } else {
            let transform = Arc::new(realize_chain(&self.ctx, input, output, type_)?);
            self.transform_cache
                .insert(search_params, Arc::downgrade(&transform));
            transform
        };

        Ok(LcmsColorTransform(transform))
    }

    fn output_transformation(
        &mut self,
        output: &Self::ColorProfile,
    ) -> Result<Self::ColorTransformation, Self::Error> {
        let search_params = SearchParams {
            input: None,
            output: output.0.profile.profile_id(),
            type_: InternalTransformType::BlendToOutput,
        };

        self.transform_cache.retain(|_, value| value.upgrade().is_some());
        let transform = if let Some(weak_transform) = self.transform_cache.get(&search_params) {
            weak_transform.upgrade().unwrap()
        } else {
            let transform = Arc::new(LcmsColorTransformInternal {
                pre_curve: Some(
                    output
                        .0
                        .output_curves(&self.ctx)
                        .ok_or(Error::BadOutputProfile)?
                        .output_inv_eotf_vcgt
                        .clone(),
                ),
                mapping: None,
                post_curve: None,
                user_data: UserDataMap::new(),
            });
            self.transform_cache
                .insert(search_params, Arc::downgrade(&transform));
            transform
        };

        Ok(LcmsColorTransform(transform))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum InternalTransformType {
    InputToBlend,
    InputToOutput,
    BlendToOutput,
}

impl From<TransformType> for InternalTransformType {
    fn from(type_: TransformType) -> Self {
        match type_ {
            TransformType::InputToBlend => InternalTransformType::InputToBlend,
            TransformType::InputToOutput => InternalTransformType::InputToOutput,
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SearchParams {
    input: Option<lcms2_sys::ProfileID>,
    output: lcms2_sys::ProfileID,
    type_: InternalTransformType,
}

impl Curve for lcms2::ToneCurve {
    fn fill_in(&self, lut: &mut [f32]) {
        let len = lut.len();
        for i in 0..len {
            let x = i as f32 / (len - 1) as f32;
            lut[i] = self.eval(x);
        }
    }
}
