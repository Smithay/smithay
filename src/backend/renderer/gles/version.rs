use std::{ffi::CStr, os::raw::c_char};

use scan_fmt::scan_fmt;

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

impl TryFrom<&CStr> for GlVersion {
    type Error = scan_fmt::parse::ScanError;

    fn try_from(value: &CStr) -> Result<Self, Self::Error> {
        scan_fmt!(&value.to_string_lossy(), "{d}.{d}", i32, i32)
            .or_else(|_| scan_fmt!(&value.to_string_lossy(), "OpenGL ES {d}.{d}", i32, i32))
            .map(|(major, minor)| GlVersion::new(major, minor))
    }
}

impl TryFrom<&Gles2> for GlVersion {
    type Error = scan_fmt::parse::ScanError;

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
    fn test_parse_mesa_3_2() {
        let gl_version = "OpenGL ES 3.2 Mesa 20.3.5";
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
