//! OS-Specific DRM constants

// DRM major value.

#[cfg(target_os = "dragonfly")]
pub const DRM_MAJOR: u64 = 145;

#[cfg(target_os = "netbsd")]
pub const DRM_MAJOR: u64 = 34;

#[cfg(all(target_os = "openbsd", target_arch = "i386"))]
pub const DRM_MAJOR: u64 = 88;

#[cfg(all(target_os = "openbsd", not(target_arch = "i386")))]
pub const DRM_MAJOR: u64 = 87;

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
#[allow(dead_code)] // Not used on Linux
pub const DRM_MAJOR: u64 = 226;

// DRM node prefixes

#[cfg(not(target_os = "openbsd"))]
pub const PRIMARY_NAME: &str = "card";

#[cfg(target_os = "freebsd")]
pub const PRIMARY_NAME: &str = "drm";

#[cfg(not(target_os = "openbsd"))]
pub const CONTROL_NAME: &str = "controlD";

#[cfg(target_os = "freebsd")]
pub const CONTROL_NAME: &str = "drmC";

#[cfg(not(target_os = "openbsd"))]
pub const RENDER_NAME: &str = "renderD";

#[cfg(target_os = "freebsd")]
pub const RENDER_NAME: &str = "drmR";
