use std::borrow::Cow;

use super::GlesError;

/// Different value types of a shader uniform variable for the [`GlesRenderer`](super::GlesRenderer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UniformType {
    /// A single float
    _1f,
    /// Two floats
    _2f,
    /// Three floats
    _3f,
    /// Four floats
    _4f,
    /// A single signed integer
    _1i,
    /// Two signed integers
    _2i,
    /// Three signed integers
    _3i,
    /// Four signed integers
    _4i,
    /// A single unsigned integer
    _1ui,
    /// Two unsigned integers
    _2ui,
    /// Three unsigned integers
    _3ui,
    /// Four unsigned integers
    _4ui,
    /// 2x2 matrices
    Matrix2x2,
    /// 2x3 matrices
    Matrix2x3,
    /// 2x4 matrices
    Matrix2x4,
    /// 3x2 matrices
    Matrix3x2,
    /// 3x3 matrices
    Matrix3x3,
    /// 3x4 matrices
    Matrix3x4,
    /// 4x2 matrices
    Matrix4x2,
    /// 4x3 matrices
    Matrix4x3,
    /// 4x4 matrices
    Matrix4x4,
}

/// GL location and type of a uniform shader variable
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct UniformDesc {
    /// GL location of the uniform
    pub location: super::ffi::types::GLint,
    /// type of the uniform
    pub type_: UniformType,
}

/// A shader uniform variable consisting out of a name and value
#[derive(Debug, Clone, PartialEq)]
pub struct Uniform<'a> {
    /// name of the uniform
    pub name: Cow<'a, str>,
    /// value of the uniform
    pub value: UniformValue,
}

/// A description of a uniform shader variable consisting out of a name and type
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniformName<'a> {
    /// name of the uniform
    pub name: Cow<'a, str>,
    /// type of the uniform
    pub type_: UniformType,
}

impl<'a> Uniform<'a> {
    /// Create a new uniform variable value
    pub fn new(name: impl Into<Cow<'a, str>>, value: impl Into<UniformValue>) -> Uniform<'a> {
        Uniform {
            name: name.into(),
            value: value.into(),
        }
    }

    /// Convert the uniform to a static lifetime, cloning the contents
    pub fn to_owned(&self) -> Uniform<'static> {
        Uniform {
            name: Cow::Owned(self.name.clone().into_owned()),
            value: self.value.clone(),
        }
    }

    /// Convert the uniform to a static lifetime, cloning the name if necessary
    pub fn into_owned(self) -> Uniform<'static> {
        Uniform {
            name: Cow::Owned(self.name.into_owned()),
            value: self.value,
        }
    }
}

impl<'a> UniformName<'a> {
    /// Create a new uniform variable description
    pub fn new(name: impl Into<Cow<'a, str>>, type_: UniformType) -> UniformName<'a> {
        UniformName {
            name: name.into(),
            type_,
        }
    }

    /// Convert the uniform name to a static lifetime, cloning the contents
    pub fn to_owned(&self) -> UniformName<'static> {
        UniformName {
            name: Cow::Owned(self.name.clone().into_owned()),
            type_: self.type_,
        }
    }

    /// Convert the uniform name to a static lifetime, cloning the name if necessary
    pub fn into_owned(self) -> UniformName<'static> {
        UniformName {
            name: Cow::Owned(self.name.into_owned()),
            type_: self.type_,
        }
    }
}

/// Value of a uniform variable
#[derive(Debug, Clone, PartialEq)]
pub enum UniformValue {
    /// A single float
    _1f(f32),
    /// Two floats
    _2f(f32, f32),
    /// Three floats
    _3f(f32, f32, f32),
    /// Four floats
    _4f(f32, f32, f32, f32),
    /// A single signed integer
    _1i(i32),
    /// Two signed integers
    _2i(i32, i32),
    /// Three signed integers
    _3i(i32, i32, i32),
    /// Four signed integers
    _4i(i32, i32, i32, i32),
    /// A single unsigned integer
    _1ui(u32),
    /// Two unsigned integers
    _2ui(u32, u32),
    /// Three signed integers
    _3ui(u32, u32, u32),
    /// Four signed integers
    _4ui(u32, u32, u32, u32),
    /// 2x2 matrices
    Matrix2x2 {
        /// Matrices supplied as [f32]-arrays
        matrices: Vec<[f32; 4]>,
        /// If transpose is `false`, each matrix is assumed to be supplied in column major order.
        /// If transpose is `true`, each matrix is assumed to be supplied in row major order.
        transpose: bool,
    },
    /// 2x3 matrices
    Matrix2x3 {
        /// Matrices supplied as [f32]-arrays
        matrices: Vec<[f32; 6]>,
        /// If transpose is `false`, each matrix is assumed to be supplied in column major order.
        /// If transpose is `true`, each matrix is assumed to be supplied in row major order.
        transpose: bool,
    },
    /// 2x4 matrices
    Matrix2x4 {
        /// Matrices supplied as [f32]-arrays
        matrices: Vec<[f32; 8]>,
        /// If transpose is `false`, each matrix is assumed to be supplied in column major order.
        /// If transpose is `true`, each matrix is assumed to be supplied in row major order.
        transpose: bool,
    },
    /// 3x2 matrices
    Matrix3x2 {
        /// Matrices supplied as [f32]-arrays
        matrices: Vec<[f32; 6]>,
        /// If transpose is `false`, each matrix is assumed to be supplied in column major order.
        /// If transpose is `true`, each matrix is assumed to be supplied in row major order.
        transpose: bool,
    },
    /// 3x3 matrices
    Matrix3x3 {
        /// Matrices supplied as [f32]-arrays
        matrices: Vec<[f32; 9]>,
        ///  If transpose is `false`, each matrix is assumed to be supplied in column major order.
        /// If transpose is `true`, each matrix is assumed to be supplied in row major order.
        transpose: bool,
    },
    /// 3x4 matrices
    Matrix3x4 {
        /// Matrices supplied as [f32]-arrays
        matrices: Vec<[f32; 12]>,
        /// If transpose is `false`, each matrix is assumed to be supplied in column major order.
        /// If transpose is `true`, each matrix is assumed to be supplied in row major order.
        transpose: bool,
    },
    /// 4x2 matrices
    Matrix4x2 {
        /// Matrices supplied as [f32]-arrays
        matrices: Vec<[f32; 8]>,
        /// If transpose is `false`, each matrix is assumed to be supplied in column major order.
        /// If transpose is `true`, each matrix is assumed to be supplied in row major order.
        transpose: bool,
    },
    /// 4x3 matrices
    Matrix4x3 {
        /// Matrices supplied as [f32]-arrays
        matrices: Vec<[f32; 12]>,
        /// If transpose is `false`, each matrix is assumed to be supplied in column major order.
        /// If transpose is `true`, each matrix is assumed to be supplied in row major order.
        transpose: bool,
    },
    /// 4x4 matrices
    Matrix4x4 {
        /// Matrices supplied as [f32]-arrays
        matrices: Vec<[f32; 16]>,
        /// If transpose is `false`, each matrix is assumed to be supplied in column major order.
        /// If transpose is `true`, each matrix is assumed to be supplied in row major order.
        transpose: bool,
    },
}

impl UniformValue {
    /// Checks if the contexts of this value match the provided uniform type
    pub fn matches(&self, type_: &UniformType) -> bool {
        *type_ == self.type_()
    }

    /// Returns the type of this uniform value
    pub fn type_(&self) -> UniformType {
        match self {
            UniformValue::_1f(_) => UniformType::_1f,
            UniformValue::_2f(_, _) => UniformType::_2f,
            UniformValue::_3f(_, _, _) => UniformType::_3f,
            UniformValue::_4f(_, _, _, _) => UniformType::_4f,
            UniformValue::_1i(_) => UniformType::_1i,
            UniformValue::_2i(_, _) => UniformType::_2i,
            UniformValue::_3i(_, _, _) => UniformType::_3i,
            UniformValue::_4i(_, _, _, _) => UniformType::_4i,
            UniformValue::_1ui(_) => UniformType::_1ui,
            UniformValue::_2ui(_, _) => UniformType::_2ui,
            UniformValue::_3ui(_, _, _) => UniformType::_3ui,
            UniformValue::_4ui(_, _, _, _) => UniformType::_4ui,
            UniformValue::Matrix2x2 { .. } => UniformType::Matrix2x2,
            UniformValue::Matrix2x3 { .. } => UniformType::Matrix2x3,
            UniformValue::Matrix2x4 { .. } => UniformType::Matrix2x4,
            UniformValue::Matrix3x2 { .. } => UniformType::Matrix3x2,
            UniformValue::Matrix3x3 { .. } => UniformType::Matrix3x3,
            UniformValue::Matrix3x4 { .. } => UniformType::Matrix3x4,
            UniformValue::Matrix4x2 { .. } => UniformType::Matrix4x2,
            UniformValue::Matrix4x3 { .. } => UniformType::Matrix4x3,
            UniformValue::Matrix4x4 { .. } => UniformType::Matrix4x4,
        }
    }

    /// Sets the `desc` uniform to this value.
    ///
    /// # Safety
    ///
    /// You have to make sure to pass a valid `UniformDesc`, and to only call this function when it
    /// is otherwise safe to call `gl.Uniform()` series of methods.
    pub unsafe fn set(&self, gl: &super::ffi::Gles2, desc: &UniformDesc) -> Result<(), GlesError> {
        if !self.matches(&desc.type_) {
            return Err(GlesError::UniformTypeMismatch {
                provided: self.type_(),
                declared: desc.type_,
            });
        }

        match self {
            UniformValue::_1f(v0) => unsafe { gl.Uniform1f(desc.location, *v0) },
            UniformValue::_2f(v0, v1) => unsafe { gl.Uniform2f(desc.location, *v0, *v1) },
            UniformValue::_3f(v0, v1, v2) => unsafe { gl.Uniform3f(desc.location, *v0, *v1, *v2) },
            UniformValue::_4f(v0, v1, v2, v3) => unsafe { gl.Uniform4f(desc.location, *v0, *v1, *v2, *v3) },
            UniformValue::_1i(v0) => unsafe { gl.Uniform1i(desc.location, *v0) },
            UniformValue::_2i(v0, v1) => unsafe { gl.Uniform2i(desc.location, *v0, *v1) },
            UniformValue::_3i(v0, v1, v2) => unsafe { gl.Uniform3i(desc.location, *v0, *v1, *v2) },
            UniformValue::_4i(v0, v1, v2, v3) => unsafe { gl.Uniform4i(desc.location, *v0, *v1, *v2, *v3) },
            UniformValue::_1ui(v0) => unsafe { gl.Uniform1ui(desc.location, *v0) },
            UniformValue::_2ui(v0, v1) => unsafe { gl.Uniform2ui(desc.location, *v0, *v1) },
            UniformValue::_3ui(v0, v1, v2) => unsafe { gl.Uniform3ui(desc.location, *v0, *v1, *v2) },
            UniformValue::_4ui(v0, v1, v2, v3) => unsafe { gl.Uniform4ui(desc.location, *v0, *v1, *v2, *v3) },
            UniformValue::Matrix2x2 { matrices, transpose } => unsafe {
                gl.UniformMatrix2fv(
                    desc.location,
                    matrices.len() as i32,
                    *transpose as u8,
                    matrices.as_ptr() as *const _,
                )
            },
            UniformValue::Matrix2x3 { matrices, transpose } => unsafe {
                gl.UniformMatrix2x3fv(
                    desc.location,
                    matrices.len() as i32,
                    *transpose as u8,
                    matrices.as_ptr() as *const _,
                )
            },
            UniformValue::Matrix2x4 { matrices, transpose } => unsafe {
                gl.UniformMatrix2x4fv(
                    desc.location,
                    matrices.len() as i32,
                    *transpose as u8,
                    matrices.as_ptr() as *const _,
                )
            },
            UniformValue::Matrix3x2 { matrices, transpose } => unsafe {
                gl.UniformMatrix3x2fv(
                    desc.location,
                    matrices.len() as i32,
                    *transpose as u8,
                    matrices.as_ptr() as *const _,
                )
            },
            UniformValue::Matrix3x3 { matrices, transpose } => unsafe {
                gl.UniformMatrix3fv(
                    desc.location,
                    matrices.len() as i32,
                    *transpose as u8,
                    matrices.as_ptr() as *const _,
                )
            },
            UniformValue::Matrix3x4 { matrices, transpose } => unsafe {
                gl.UniformMatrix3x4fv(
                    desc.location,
                    matrices.len() as i32,
                    *transpose as u8,
                    matrices.as_ptr() as *const _,
                )
            },
            UniformValue::Matrix4x2 { matrices, transpose } => unsafe {
                gl.UniformMatrix4x2fv(
                    desc.location,
                    matrices.len() as i32,
                    *transpose as u8,
                    matrices.as_ptr() as *const _,
                )
            },
            UniformValue::Matrix4x3 { matrices, transpose } => unsafe {
                gl.UniformMatrix4x3fv(
                    desc.location,
                    matrices.len() as i32,
                    *transpose as u8,
                    matrices.as_ptr() as *const _,
                )
            },
            UniformValue::Matrix4x4 { matrices, transpose } => unsafe {
                gl.UniformMatrix4fv(
                    desc.location,
                    matrices.len() as i32,
                    *transpose as u8,
                    matrices.as_ptr() as *const _,
                )
            },
        };

        Ok(())
    }
}

impl From<f32> for UniformValue {
    #[inline]
    fn from(v: f32) -> Self {
        UniformValue::_1f(v)
    }
}
impl From<(f32, f32)> for UniformValue {
    #[inline]
    fn from((v1, v2): (f32, f32)) -> Self {
        UniformValue::_2f(v1, v2)
    }
}
impl From<(f32, f32, f32)> for UniformValue {
    #[inline]
    fn from((v1, v2, v3): (f32, f32, f32)) -> Self {
        UniformValue::_3f(v1, v2, v3)
    }
}
impl From<(f32, f32, f32, f32)> for UniformValue {
    #[inline]
    fn from((v1, v2, v3, v4): (f32, f32, f32, f32)) -> Self {
        UniformValue::_4f(v1, v2, v3, v4)
    }
}
impl From<[f32; 2]> for UniformValue {
    #[inline]
    fn from(v: [f32; 2]) -> Self {
        UniformValue::_2f(v[0], v[1])
    }
}
impl From<[f32; 3]> for UniformValue {
    #[inline]
    fn from(v: [f32; 3]) -> Self {
        UniformValue::_3f(v[0], v[1], v[2])
    }
}
impl From<[f32; 4]> for UniformValue {
    #[inline]
    fn from(v: [f32; 4]) -> Self {
        UniformValue::_4f(v[0], v[1], v[2], v[3])
    }
}

impl From<i32> for UniformValue {
    #[inline]
    fn from(v: i32) -> Self {
        UniformValue::_1i(v)
    }
}
impl From<(i32, i32)> for UniformValue {
    #[inline]
    fn from((v1, v2): (i32, i32)) -> Self {
        UniformValue::_2i(v1, v2)
    }
}
impl From<(i32, i32, i32)> for UniformValue {
    #[inline]
    fn from((v1, v2, v3): (i32, i32, i32)) -> Self {
        UniformValue::_3i(v1, v2, v3)
    }
}
impl From<(i32, i32, i32, i32)> for UniformValue {
    #[inline]
    fn from((v1, v2, v3, v4): (i32, i32, i32, i32)) -> Self {
        UniformValue::_4i(v1, v2, v3, v4)
    }
}
impl From<[i32; 2]> for UniformValue {
    #[inline]
    fn from(v: [i32; 2]) -> Self {
        UniformValue::_2i(v[0], v[1])
    }
}
impl From<[i32; 3]> for UniformValue {
    #[inline]
    fn from(v: [i32; 3]) -> Self {
        UniformValue::_3i(v[0], v[1], v[2])
    }
}
impl From<[i32; 4]> for UniformValue {
    #[inline]
    fn from(v: [i32; 4]) -> Self {
        UniformValue::_4i(v[0], v[1], v[2], v[3])
    }
}

impl From<u32> for UniformValue {
    #[inline]
    fn from(v: u32) -> Self {
        UniformValue::_1ui(v)
    }
}
impl From<(u32, u32)> for UniformValue {
    #[inline]
    fn from((v1, v2): (u32, u32)) -> Self {
        UniformValue::_2ui(v1, v2)
    }
}
impl From<(u32, u32, u32)> for UniformValue {
    #[inline]
    fn from((v1, v2, v3): (u32, u32, u32)) -> Self {
        UniformValue::_3ui(v1, v2, v3)
    }
}
impl From<(u32, u32, u32, u32)> for UniformValue {
    #[inline]
    fn from((v1, v2, v3, v4): (u32, u32, u32, u32)) -> Self {
        UniformValue::_4ui(v1, v2, v3, v4)
    }
}
impl From<[u32; 2]> for UniformValue {
    #[inline]
    fn from(v: [u32; 2]) -> Self {
        UniformValue::_2ui(v[0], v[1])
    }
}
impl From<[u32; 3]> for UniformValue {
    #[inline]
    fn from(v: [u32; 3]) -> Self {
        UniformValue::_3ui(v[0], v[1], v[2])
    }
}
impl From<[u32; 4]> for UniformValue {
    #[inline]
    fn from(v: [u32; 4]) -> Self {
        UniformValue::_4ui(v[0], v[1], v[2], v[3])
    }
}
