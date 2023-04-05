#![allow(missing_docs)]

#[cfg(feature = "color_lcms")]
pub mod lcms;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransformType {
    InputToBlend,
    BlendToOutput,
    InputToOutput,
}

pub trait CMS {
    type Error: std::error::Error;
    type ColorProfile: std::hash::Hash;
    type ColorTransformation: Transformation;

    fn profile_srgb(&self) -> Self::ColorProfile;
    fn profile_from_icc(&mut self, icc: &[u8]) -> Result<Self::ColorProfile, Self::Error>;
    fn transformation(
        &mut self,
        input: &Self::ColorProfile,
        output: &Self::ColorProfile,
        type_: TransformType,
    ) -> Result<Self::ColorTransformation, Self::Error>;
}
