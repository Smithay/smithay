use std::{
    ffi::{CStr, CString},
    os::raw::c_char,
};

use super::ffi::{self, Gles2};

pub const GLES_3_0: GlVersion = GlVersion::new(3, 0);
pub const GLES_2_0: GlVersion = GlVersion::new(2, 0);

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct GlVersion {
    pub major: i32,
    pub minor: i32,
}

impl GlVersion {
    pub const fn new(major: i32, minor: i32) -> Self {
        GlVersion { major, minor }
    }
}

impl Eq for GlVersion {}

impl Ord for GlVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.major.cmp(&other.major) {
            std::cmp::Ordering::Equal => self.minor.cmp(&other.minor),
            ord => ord,
        }
    }
}

impl PartialOrd for GlVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid version string {0:?}")]
pub struct GlVersionParseError(CString);

impl TryFrom<&CStr> for GlVersion {
    type Error = GlVersionParseError;

    fn try_from(value: &CStr) -> Result<Self, Self::Error> {
        let mut bytes = value.to_bytes();

        let prefix = b"OpenGL ES ";
        if bytes.starts_with(prefix) {
            // Strip the prefix
            bytes = &bytes[prefix.len()..];
        }

        let ascii_to_int = |ch: u8| (ch - b'0') as u32;

        let mut iter = bytes.iter();

        let mut major: Option<u32> = None;
        let mut minor: Option<u32> = None;

        for v in &mut iter {
            if v.is_ascii_digit() {
                major = Some(major.unwrap_or(0) * 10 + ascii_to_int(*v));
            } else if *v == b'.' {
                break;
            } else {
                // Neither digit nor '.', so let's assume invalid string
                return Err(GlVersionParseError(value.to_owned()));
            }
        }

        for v in iter {
            if v.is_ascii_digit() {
                minor = Some(minor.unwrap_or(0) * 10 + ascii_to_int(*v));
            } else {
                break;
            }
        }

        major
            .zip(minor)
            .map(|(major, minor)| GlVersion::new(major as i32, minor as i32))
            .ok_or_else(|| GlVersionParseError(value.to_owned()))
    }
}

impl TryFrom<&Gles2> for GlVersion {
    type Error = GlVersionParseError;

    fn try_from(value: &Gles2) -> Result<Self, Self::Error> {
        let version = unsafe { CStr::from_ptr(value.GetString(ffi::VERSION) as *const c_char) };
        GlVersion::try_from(version)
    }
}

#[cfg(test)]
mod tests {
    use super::GlVersion;
    use std::{convert::TryFrom, ffi::CStr, os::raw::c_char};

    #[test]
    fn test_parse_1234_4321() {
        let gl_version = b"1234.4321 Mesa 20.3.5\0";
        let gl_version_str = unsafe { CStr::from_ptr(gl_version.as_ptr() as *const c_char) };
        assert_eq!(
            GlVersion::try_from(gl_version_str).unwrap(),
            GlVersion::new(1234, 4321)
        )
    }

    #[test]
    fn test_parse_mesa_3_2() {
        let gl_version = b"OpenGL ES 3.2 Mesa 20.3.5\0";
        let gl_version_str = unsafe { CStr::from_ptr(gl_version.as_ptr() as *const c_char) };
        assert_eq!(GlVersion::try_from(gl_version_str).unwrap(), GlVersion::new(3, 2))
    }

    #[test]
    fn test_3_2_greater_3_0() {
        assert!(GlVersion::new(3, 2) > GlVersion::new(3, 0))
    }

    #[test]
    fn test_3_0_greater_or_equal_3_0() {
        assert!(GlVersion::new(3, 0) >= GlVersion::new(3, 0))
    }

    #[test]
    fn test_3_0_less_or_equal_3_0() {
        assert!(GlVersion::new(3, 0) <= GlVersion::new(3, 0))
    }

    #[test]
    fn test_3_0_eq_3_0() {
        assert!(GlVersion::new(3, 0) == GlVersion::new(3, 0))
    }

    #[test]
    fn test_2_0_less_3_0() {
        assert!(GlVersion::new(2, 0) < GlVersion::new(3, 0))
    }
}
