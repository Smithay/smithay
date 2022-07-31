//! Functions to get physical device information

use std::ffi::{CStr, CString};

use ash::{prelude::VkResult, vk};

use super::{version::Version, DriverInfo, PhdInfo};

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
            Ok(Some(Self {
                phd,
                info,
                extensions,
                instance,
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
        let features = unsafe { instance.get_physical_device_features(phd) };

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

        // Maintenance3
        //
        // SAFETY: Maintenance3 extension is supported since Vulkan 1.1
        let maintenance_3 = unsafe {
            let mut maintenance_3 = vk::PhysicalDeviceMaintenance3Properties::default();
            let mut props = vk::PhysicalDeviceProperties2::builder().push_next(&mut maintenance_3);
            instance.get_physical_device_properties2(phd, &mut props);
            maintenance_3
        };

        // IDProperties
        //
        // SAFETY: Core in Vulkan 1.1
        let id = unsafe {
            let mut id = vk::PhysicalDeviceIDProperties::default();
            let mut props = vk::PhysicalDeviceProperties2::builder().push_next(&mut id);
            instance.get_physical_device_properties2(phd, &mut props);
            id
        };

        // Vulkan 1.1 features
        //
        // Confusingly these types were not added until Vulkan 1.2
        let (properties_1_1, features_1_1) = {
            if api_version >= Version::VERSION_1_2 {
                unsafe {
                    let mut properties_1_1 = vk::PhysicalDeviceVulkan11Properties::default();
                    let mut props = vk::PhysicalDeviceProperties2::builder().push_next(&mut properties_1_1);
                    instance.get_physical_device_properties2(phd, &mut props);

                    let mut features_1_1 = vk::PhysicalDeviceVulkan11Features::default();
                    let mut features = vk::PhysicalDeviceFeatures2::builder().push_next(&mut features_1_1);
                    instance.get_physical_device_features2(phd, &mut features);

                    (Some(properties_1_1), Some(features_1_1))
                }
            } else {
                (None, None)
            }
        };

        // Vulkan 1.2
        let (properties_1_2, features_1_2) = {
            if api_version >= Version::VERSION_1_2 {
                // SAFETY: The physical device supports Vulkan 1.2
                unsafe {
                    let mut properties_1_2 = vk::PhysicalDeviceVulkan12Properties::default();
                    let mut props = vk::PhysicalDeviceProperties2::builder().push_next(&mut properties_1_2);
                    instance.get_physical_device_properties2(phd, &mut props);

                    let mut features_1_2 = vk::PhysicalDeviceVulkan12Features::default();
                    let mut features = vk::PhysicalDeviceFeatures2::builder().push_next(&mut features_1_2);
                    instance.get_physical_device_features2(phd, &mut features);

                    (Some(properties_1_2), Some(features_1_2))
                }
            } else {
                (None, None)
            }
        };

        // Vulkan 1.3
        let (properties_1_3, features_1_3) = {
            if api_version >= Version::VERSION_1_3 {
                // SAFETY: The physical device supports Vulkan 1.3
                unsafe {
                    let mut properties_1_2 = vk::PhysicalDeviceVulkan13Properties::default();
                    let mut props = vk::PhysicalDeviceProperties2::builder().push_next(&mut properties_1_2);
                    instance.get_physical_device_properties2(phd, &mut props);

                    let mut features_1_2 = vk::PhysicalDeviceVulkan13Features::default();
                    let mut features = vk::PhysicalDeviceFeatures2::builder().push_next(&mut features_1_2);
                    instance.get_physical_device_features2(phd, &mut features);

                    (Some(properties_1_2), Some(features_1_2))
                }
            } else {
                (None, None)
            }
        };

        // VK_EXT_physical_device_drm
        let properties_drm = if supported_extensions
            .iter()
            .any(|name| name.as_c_str() == vk::ExtPhysicalDeviceDrmFn::name())
        {
            // SAFETY: The caller has garunteed the physical device supports VK_EXT_physical_device_drm
            unsafe {
                let mut properties_drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
                let mut props = vk::PhysicalDeviceProperties2::builder().push_next(&mut properties_drm);
                instance.get_physical_device_properties2(phd, &mut props);
                Some(properties_drm)
            }
        } else {
            None
        };

        // VK_KHR_driver_properties or Vulkan 1.2
        let driver = if api_version >= Version::VERSION_1_2 {
            // Copy the data from the Vulkan 1.2 properties into the extension struct for data creation.
            let properties = vk::PhysicalDeviceDriverProperties {
                driver_id: properties_1_2.unwrap().driver_id,
                driver_name: properties_1_2.unwrap().driver_name,
                driver_info: properties_1_2.unwrap().driver_info,
                conformance_version: properties_1_2.unwrap().conformance_version,
                ..Default::default()
            };

            Some(unsafe { DriverInfo::from_driver_properties(properties) })
        } else if supported_extensions
            .iter()
            .any(|name| name.as_c_str() == vk::KhrDriverPropertiesFn::name())
        {
            // SAFETY: VK_KHR_driver_properties is supported
            unsafe {
                let mut driver_props = vk::PhysicalDeviceDriverProperties::default();
                let mut props = vk::PhysicalDeviceProperties2::builder().push_next(&mut driver_props);
                instance.get_physical_device_properties2(phd, &mut props);

                Some(DriverInfo::from_driver_properties(driver_props))
            }
        } else {
            None
        };

        Some(Self {
            api_version,
            name,
            properties,
            features,
            maintenance_3,
            id,
            properties_1_1,
            features_1_1,
            properties_1_2,
            features_1_2,
            properties_1_3,
            features_1_3,
            properties_drm,
            driver,
        })
    }
}

impl super::DriverInfo {
    unsafe fn from_driver_properties(properties: vk::PhysicalDeviceDriverProperties) -> DriverInfo {
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
