use once_cell::sync::Lazy;
use std::convert::Infallible;

use super::*;

static GLOBAL_USER_DATA: Lazy<UserDataMap> = Lazy::new(UserDataMap::new);

#[derive(Debug, Clone, Copy)]
pub struct NullCMS;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NullProfile;
#[derive(Debug, Clone, Copy)]
pub struct IdentityTransformation;

impl CMS for NullCMS {
    type Error = Infallible;
    type ColorProfile = NullProfile;
    type ColorTransformation = IdentityTransformation;

    fn profile_srgb(&self) -> Self::ColorProfile {
        NullProfile
    }
    fn profile_from_icc(&mut self, _icc: &[u8]) -> Result<Self::ColorProfile, Self::Error> {
        Ok(NullProfile)
    }
    fn input_transformation(
        &mut self,
        _input: &Self::ColorProfile,
        _output: &Self::ColorProfile,
        _type_: TransformType,
    ) -> Result<Self::ColorTransformation, Self::Error> {
        Ok(IdentityTransformation)
    }
    fn output_transformation(
        &mut self,
        _output: &Self::ColorProfile,
    ) -> Result<Self::ColorTransformation, Self::Error> {
        Ok(IdentityTransformation)
    }
}

pub struct Unreachable;
impl Curve for Unreachable {
    fn fill_in(&self, _lut: &mut [f32]) {}
}
impl MappingLUT for Unreachable {
    fn fill_in(&self, _lut: &mut [f32], _len: usize) {}
}

impl Transformation for IdentityTransformation {
    type Curve = Unreachable;
    type MappingLUT = Unreachable;

    fn pre_curve(&self) -> Option<&[Self::Curve; 3]> {
        None
    }
    fn mapping(&self) -> Option<&Mapping<Self::MappingLUT>> {
        None
    }
    fn post_curve(&self) -> Option<&[Self::Curve; 3]> {
        None
    }

    fn user_data(&self) -> &UserDataMap {
        &*GLOBAL_USER_DATA
    }
}
