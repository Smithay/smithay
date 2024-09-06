//! OS-Specific DRM constants

// DRM major value.

#[cfg(target_os = "dragonfly")]
pub const DRM_MAJOR: u32 = 145;

#[cfg(target_os = "netbsd")]
pub const DRM_MAJOR: u32 = 180;

#[cfg(all(target_os = "openbsd", target_arch = "x86"))]
pub const DRM_MAJOR: u32 = 88;

#[cfg(all(target_os = "openbsd", not(target_arch = "x86")))]
pub const DRM_MAJOR: u32 = 87;

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
#[allow(dead_code)] // Not used on Linux
pub const DRM_MAJOR: u32 = 226;

// DRM node prefixes

#[cfg(not(target_os = "openbsd"))]
pub const PRIMARY_NAME: &str = "card";

#[cfg(target_os = "openbsd")]
pub const PRIMARY_NAME: &str = "drm";

#[cfg(not(target_os = "openbsd"))]
pub const CONTROL_NAME: &str = "controlD";

#[cfg(target_os = "openbsd")]
pub const CONTROL_NAME: &str = "drmC";

#[cfg(not(target_os = "openbsd"))]
pub const RENDER_NAME: &str = "renderD";

#[cfg(target_os = "openbsd")]
pub const RENDER_NAME: &str = "drmR";
