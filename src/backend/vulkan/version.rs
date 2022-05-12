//! The [`Version`] type.

use std::{
    cmp::Ordering,
    fmt::{self, Formatter},
};

use ash::vk;

/// A Vulkan API version.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Version {
    /// The variant of the Vulkan API.
    ///
    /// Generally this value will be `0` because the Vulkan specification uses variant `0`.
    pub variant: u32,

    /// The major version of the Vulkan API.
    pub major: u32,

    /// The minor version of the Vulkan API.
    pub minor: u32,

    /// The patch version of the Vulkan API.
    ///
    /// Most Vulkan API calls which take a version typically ignore the patch value. Consumers of the Vulkan API may
    /// typically ignore the patch value.
    pub patch: u32,
}

impl Version {
    /// Version 1.0 of the Vulkan API.
    pub const VERSION_1_0: Version = Version::from_raw(vk::API_VERSION_1_0);

    /// Version 1.1 of the Vulkan API.
    pub const VERSION_1_1: Version = Version::from_raw(vk::API_VERSION_1_1);

    /// Version 1.2 of the Vulkan API.
    pub const VERSION_1_2: Version = Version::from_raw(vk::API_VERSION_1_2);

    /// Version 1.3 of the Vulkan API.
    pub const VERSION_1_3: Version = Version::from_raw(vk::API_VERSION_1_3);

    /// The version of Smithay.
    pub const SMITHAY: Version = Version {
        // TODO: May be useful to place the version information in a single spot that isn't just Vulkan
        variant: 0,
        major: 0,
        minor: 3,
        patch: 0,
    };

    /// Converts a packed version into a version struct.
    pub const fn from_raw(raw: u32) -> Version {
        Version {
            variant: vk::api_version_variant(raw),
            major: vk::api_version_major(raw),
            minor: vk::api_version_minor(raw),
            patch: vk::api_version_patch(raw),
        }
    }

    /// Converts a version struct into a packed version.
    pub const fn to_raw(self) -> u32 {
        vk::make_api_version(self.variant, self.major, self.minor, self.patch)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{}.{} variant {}",
            self.major, self.minor, self.patch, self.variant
        )
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.variant.cmp(&other.variant) {
            Ordering::Equal => {}
            ord => return ord,
        }

        match self.major.cmp(&other.major) {
            Ordering::Equal => {}
            ord => return ord,
        }

        match self.minor.cmp(&other.minor) {
            Ordering::Equal => {}
            ord => return ord,
        }

        self.patch.cmp(&other.patch)
    }
}
