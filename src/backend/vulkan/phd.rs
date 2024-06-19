//! Functions to get physical device information

use std::ffi::{CStr, CString};

use ash::{ext, khr, prelude::VkResult, vk};
use tracing::info_span;

use super::{version::Version, DriverInfo, PhdInfo, UnsupportedProperty};

impl super::PhysicalDevice {
    /// # Safety:
    ///
    /// The physical device must belong to the specified instance.
    pub(super) unsafe fn from_phd(
        instance: &super::Instance,
        phd: vk::PhysicalDevice,
    ) -> VkResult<Option<super::PhysicalDevice>> {
        let instance = instance.clone();

        let extensions = unsafe { instance.handle().enumerate_device_extension_properties(phd) }?;
        let extensions = extensions
            .iter()
            .map(|extension| {
                // SAFETY: Vulkan guarantees the device name is valid UTF-8 with a null terminator.
                unsafe { CStr::from_ptr(&extension.extension_name as *const _) }.to_owned()
            })
            .collect::<Vec<_>>();

        if let Some(info) =
            unsafe { PhdInfo::from_phd(instance.handle(), instance.api_version(), phd, &extensions) }
        {
            let span = info_span!(parent: &instance.0.span, "backend_vulkan_device", name = info.name);
            Ok(Some(Self {
                phd,
                info,
                extensions,
                instance,
                span,
            }))
        } else {
            Ok(None)
        }
    }
}

impl super::PhdInfo {
    /// Returns [`None`] if the physical device does not support Vulkan 1.1
    ///
    /// # Panics
    ///
    /// - If the instance version is not at least Vulkan 1.1
    ///
    /// # Safety
    ///
    /// - The instance version must be the same version the instance was created with.
    /// - The physical device must belong to the specified instance.
    unsafe fn from_phd(
        instance: &ash::Instance,
        instance_version: Version,
        phd: vk::PhysicalDevice,
        supported_extensions: &[CString],
    ) -> Option<Self> {
        assert!(instance_version >= Version::VERSION_1_1);

        let properties = unsafe { instance.get_physical_device_properties(phd) };

        // Pick the lower of the instance version and device version to get the actual version of Vulkan that
        // can be used with the device.
        let api_version = Version::from_raw(u32::min(properties.api_version, instance_version.to_raw()));

        if api_version < Version::VERSION_1_1 {
            // Device does not support Vulkan 1.1, so ignore it.
            return None;
        }

        // SAFETY: Vulkan guarantees the device name is valid UTF-8 with a null terminator.
        let name = unsafe { CStr::from_ptr(&properties.device_name as *const _) }
            .to_str()
            .unwrap()
            .to_string();

        // Initialize the type with the api_version.
        let mut info = PhdInfo {
            api_version,
            name,
            ..Default::default()
        };

        let mut properties = vk::PhysicalDeviceProperties2::default();

        // Maintenance3 and IDProperties are both Core in Vulkan 1.1
        //
        // SAFETY: Maintenance3 extension is supported since Vulkan 1.1
        properties = properties
            .push_next(&mut info.maintenance_3)
            .push_next(&mut info.id);

        // VK_EXT_physical_device_drm
        if supported_extensions
            .iter()
            .any(|name| name.as_c_str() == ext::physical_device_drm::NAME)
        {
            // SAFETY: The caller has garunteed the physical device supports VK_EXT_physical_device_drm
            let next = info
                .properties_drm
                .insert(vk::PhysicalDeviceDrmPropertiesEXT::default());
            properties = properties.push_next(next);
        }

        // VK_KHR_driver_properties or Vulkan 1.2
        if api_version >= Version::VERSION_1_2
            || supported_extensions
                .iter()
                .any(|name| name.as_c_str() == khr::driver_properties::NAME)
        {
            // SAFETY: VK_KHR_driver_properties is supported
            let next = info
                .properties_driver
                .insert(vk::PhysicalDeviceDriverProperties::default());
            properties = properties.push_next(next);
        }

        unsafe { instance.get_physical_device_properties2(phd, &mut properties) };

        info.properties = properties.properties;
        // Initialize the driver info
        info.driver = info.properties_driver.map(DriverInfo::from_driver_properties);

        Some(info)
    }

    #[cfg_attr(not(feature = "backend_drm"), allow(dead_code))]
    pub(super) fn get_drm_properties(
        &self,
    ) -> Result<vk::PhysicalDeviceDrmPropertiesEXT<'_>, UnsupportedProperty> {
        const EXTENSIONS: &[&CStr] = &[ext::physical_device_drm::NAME];
        self.properties_drm
            .ok_or(UnsupportedProperty::Extensions(EXTENSIONS))
    }
}

impl super::DriverInfo {
    fn from_driver_properties(properties: vk::PhysicalDeviceDriverProperties<'_>) -> DriverInfo {
        // SAFETY: Vulkan guarantees the driver name is valid UTF-8 with a null terminator.
        let name = unsafe { CStr::from_ptr(&properties.driver_name as *const _) }
            .to_str()
            .unwrap()
            .to_string();

        // SAFETY: Vulkan guarantees the driver info is valid UTF-8 with a null terminator.
        let info = unsafe { CStr::from_ptr(&properties.driver_info as *const _) }
            .to_str()
            .unwrap()
            .to_string();

        DriverInfo {
            id: properties.driver_id,
            name,
            info,
            conformance: properties.conformance_version,
        }
    }
}
