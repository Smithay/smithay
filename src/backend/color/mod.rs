#![allow(missing_docs)]

use crate::utils::user_data::UserDataMap;

#[cfg(feature = "color_lcms")]
pub mod lcms;
pub mod null;

// TODO, find a better spot for this?
pub const _1D_POINTS: usize = 1024;
pub const _3D_POINTS: usize = 33;

pub trait Curve {
    fn fill_in(&self, lut: &mut [f32]);
}

pub trait MappingLUT {
    fn fill_in(&self, lut: &mut [f32], len: usize);
}

#[derive(Debug, Clone)]
pub enum Mapping<LUT: MappingLUT> {
    Matrix(cgmath::Matrix3<f32>),
    LUT(LUT),
}

pub trait Transformation {
    type Curve: Curve;
    type MappingLUT: MappingLUT;

    fn pre_curve(&self) -> Option<&[Self::Curve; 3]>;
    fn mapping(&self) -> Option<&Mapping<Self::MappingLUT>>;
    fn post_curve(&self) -> Option<&[Self::Curve; 3]>;

    fn is_identity(&self) -> bool {
        self.pre_curve().is_none() && self.mapping().is_none() && self.post_curve().is_none()
    }

    fn user_data(&self) -> &UserDataMap;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransformType {
    InputToBlend,
    InputToOutput,
}

pub trait CMS {
    type Error: std::error::Error + Send + Sync + 'static;
    type ColorProfile: std::clone::Clone + std::cmp::PartialEq + std::hash::Hash;
    type ColorTransformation: Transformation;

    fn profile_srgb(&self) -> Self::ColorProfile;
    fn profile_from_icc(&mut self, icc: &[u8]) -> Result<Self::ColorProfile, Self::Error>;
    fn input_transformation(
        &mut self,
        input: &Self::ColorProfile,
        output: &Self::ColorProfile,
        type_: TransformType,
    ) -> Result<Self::ColorTransformation, Self::Error>;
    fn output_transformation(
        &mut self,
        output: &Self::ColorProfile,
    ) -> Result<Self::ColorTransformation, Self::Error>;
}
