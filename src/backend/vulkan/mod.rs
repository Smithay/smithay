//! Types to initialize and use Vulkan.
//!
//! This module provides some thin abstractions over [`ash`](https://crates.io/crates/ash) for initializing
//! Vulkan.
//!
//! This module does not provide abstractions for logical devices, rendering or memory allocation. These
//! should instead be provided in higher level abstractions.
//!
//! Smithay requires at least Vulkan 1.1[^version].
//!
//! # [`Instance`]
//!
//! To use Vulkan, you would first instantiate an [`Instance`]. An instance is effectively the Vulkan library
//! and provides some information about the environment. This includes the list of supported
//! [instance extensions](Instance::enumerate_extensions) and the list of available physical devices.
//!
//! An instance is constructed using an [`Instance::new`] or [`Instance::with_extensions`].
//!
//! ## Layers
//!
//! The validation layers will be enabled if debug assertions are enabled and the validation layers are
//! available on your system.
//!
//! ## Instance extensions
//!
//! Instances may be created with some enabled extensions. Note that any features gated by an extension are
//! only available if the extension (and it's dependencies) are enabled (you will get a validation error if
//! you ignore that).
//!
//! Some features such as window system integration are only available if their features are enabled.
//! Available instance extensions may be obtained using [`Instance::enumerate_extensions`].
//!
//! # [`PhysicalDevice`]
//!
//! Once you have an instance, you may want to find a suitable device to use. A [`PhysicalDevice`] describes a
//! Vulkan implementation that may correspond to a real or virtual device.
//!
//! To get all the available devices, use [`PhysicalDevice::enumerate`].
//!
//! Physical devices are also describe the logical devices that can be created. A physical device can describe
//! a variety of properties that may be used for device selection, including but not limited to:
//! - [Device name](PhysicalDevice::name)
//! - [Supported Vulkan API version](PhysicalDevice::api_version)
//! - [Type](PhysicalDevice::ty) of the device
//! - [Driver information](PhysicalDevice::driver)
//! - [Extensions](PhysicalDevice::device_extensions)
//! - [Features](PhysicalDevice::features) and [limits](PhysicalDevice::limits)
//!
//! Physical devices implement [`Eq`][^device_eq], meaning two physical devices can be tested for equality.
//!
//! ## Device extensions
//!
//! Depending on the device extension (see the Vulkan specification), a device extension may indicate some
//! physical device data is available or indicate some device feature is supported. Any features that are
//! added using a device extension must be enabled in order to be used.
//!
//! [^version]: Internally Vulkan 1.1 is required because several extensions that were made part of core are
//! used quite extensively in some of the extensions our abstractions use. The vast majority of systems using
//! Vulkan also support at least Vulkan 1.1. If you need Vulkan 1.0 support, please open an issue and we can
//! discuss Vulkan 1.0 support.
//!
//! [^device_eq]: Two physical devices are only equal if both physical devices are created from the same
//! instance and have the same physical device handle.

#![warn(missing_debug_implementations)]
#![forbid(unsafe_op_in_unsafe_fn)]

use std::{
    env::{self, VarError},
    ffi::CStr,
    sync::LazyLock,
};

use ash::{vk, Entry};
use libc::c_void;
use tracing::{error, info, trace, warn};

pub mod device;
pub mod format;
pub mod image;
pub mod instance;
pub mod phd;
pub mod version;

pub use self::{
    device::Device, format::FormatList, instance::Instance, phd::PhysicalDevice, version::Version,
};

static LIBRARY: LazyLock<Result<Entry, LoadError>> =
    LazyLock::new(|| unsafe { Entry::load().map_err(|_| LoadError) });

/// Error loading the Vulkan library
#[derive(Debug, thiserror::Error)]
#[error("Failed to load the Vulkan library")]
pub struct LoadError;

/// Error returned when a physical device property is not supported
#[derive(Debug, thiserror::Error)]
pub enum UnsupportedProperty {
    /// Some required extensions are not available.
    #[error("The following extensions are not available {0:?}")]
    Extensions(&'static [&'static CStr]),
}

fn get_env_or_max_version(max_version: Version) -> Version {
    // Consider max version overrides from env
    match env::var("SMITHAY_VK_VERSION") {
        Ok(version) => {
            let overridden_version = match &version[..] {
                "1.0" => {
                    warn!("Smithay does not support Vulkan 1.0, ignoring SMITHAY_VK_VERSION");
                    return max_version;
                }
                "1.1" => Some(Version::VERSION_1_1),
                "1.2" => Some(Version::VERSION_1_2),
                "1.3" => Some(Version::VERSION_1_3),
                _ => None,
            };

            // The env var can only lower the maximum version, not raise it.
            if let Some(overridden_version) = overridden_version {
                if overridden_version > max_version {
                    warn!(
                        "Ignoring SMITHAY_VK_VERSION since the requested max version is higher than the maximum of {}.{}",
                        max_version.major,
                        max_version.minor
                    );
                    max_version
                } else {
                    overridden_version
                }
            } else {
                warn!("SMITHAY_VK_VERSION was set to an unknown Vulkan version");
                max_version
            }
        }

        Err(VarError::NotUnicode(_)) => {
            warn!("Value of SMITHAY_VK_VERSION is not valid Unicode, ignoring.");

            max_version
        }

        Err(VarError::NotPresent) => max_version,
    }
}

unsafe extern "system" fn vulkan_debug_utils_callback(
    message_severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    message_type: vk::DebugUtilsMessageTypeFlagsEXT,
    p_callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    span: *mut c_void,
) -> vk::Bool32 {
    let _ = std::panic::catch_unwind(|| {
        // Get the span from the user data pointer we gave to Vulkan.
        //
        // The span is allocated on the heap using a box, but we do not want to drop the span,
        // so read from the pointer.
        let _guard = unsafe { (span as *mut tracing::Span).as_ref() }.unwrap().enter();

        // VUID-VkDebugUtilsMessengerCallbackDataEXT-pMessage-parameter: Message must be valid UTF-8 with a null
        // terminator.
        let message = unsafe { CStr::from_ptr((*p_callback_data).p_message) }.to_string_lossy();
        // Message type is in full uppercase since we print the bitflag debug representation.
        let ty = format!("{message_type:?}").to_lowercase();

        match message_severity {
            vk::DebugUtilsMessageSeverityFlagsEXT::VERBOSE => {
                trace!(ty, "{message}")
            }
            vk::DebugUtilsMessageSeverityFlagsEXT::INFO => info!(ty, "{message}"),
            vk::DebugUtilsMessageSeverityFlagsEXT::WARNING => warn!(ty, "{message}"),
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR => error!(ty, "{message}"),
            _ => (),
        }
    });

    // Must always return false.
    vk::FALSE
}

#[cfg(test)]
mod tests {
    use super::{Instance, PhysicalDevice};

    fn is_send_sync<T: Send + Sync>() {}

    /// Test that both [`Instance`] and [`PhysicalDevice`] are Send and Sync.
    #[test]
    fn send_sync() {
        is_send_sync::<Instance>();
        is_send_sync::<PhysicalDevice>();
    }
}
