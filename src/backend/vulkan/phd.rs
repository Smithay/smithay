//! Helper for VkPhysicalDevice.
//!
//! Once you have an [instance](super::Instance), you may want to find a suitable device to use. A [`PhysicalDevice`] describes a
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

use std::ffi::{CStr, CString};

use ash::{
    ext, khr,
    prelude::VkResult,
    vk::{self, PhysicalDeviceDriverProperties, PhysicalDeviceDrmPropertiesEXT},
};
#[cfg(feature = "backend_drm")]
use drm::node::DrmNode;
use tracing::info_span;
#[cfg(feature = "backend_drm")]
use tracing::instrument;

use super::{version::Version, Instance, UnsupportedProperty};

/// A Vulkan physical device.
///
/// A physical device refers to a Vulkan implementation. A physical device has no associated resources and may
/// be used to create a logical device.
#[derive(Debug, Clone)]
pub struct PhysicalDevice {
    phd: vk::PhysicalDevice,
    info: PhdInfo,
    extensions: Vec<CString>,
    instance: Instance,
    span: tracing::Span,
}

impl PhysicalDevice {
    /// Enumerates over all physical devices available on the system, returning an iterator of [`PhysicalDevice`]
    pub fn enumerate(instance: &Instance) -> VkResult<impl Iterator<Item = PhysicalDevice>> {
        let _span = instance.0.span.enter();

        // Must clone instance or else the returned iterator has a lifetime over `&Instance`
        let instance = instance.clone();
        let devices = unsafe { instance.handle().enumerate_physical_devices() }?;
        let devices = devices
            .into_iter()
            // TODO: Warn if any physical devices have an error when getting device properties.
            .flat_map(move |phd| unsafe { PhysicalDevice::from_phd(&instance, phd) })
            .flatten();

        Ok(devices)
    }

    /// # Safety:
    ///
    /// The physical device must belong to the specified instance.
    pub(super) unsafe fn from_phd(
        instance: &super::Instance,
        phd: vk::PhysicalDevice,
    ) -> VkResult<Option<PhysicalDevice>> {
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

    /// Returns the name of the device.
    pub fn name(&self) -> &str {
        &self.info.name
    }

    /// Returns the version of Vulkan supported by this device.
    ///
    /// Unlike the `api_version` property, which is the version reported by the device directly, this function
    /// returns the version the device can actually support, based on the instance’s, `api_version`.
    ///
    /// The Vulkan specification provides more information about the version requirements: <https://www.khronos.org/registry/vulkan/specs/1.3-extensions/html/vkspec.html#fundamentals-validusage-versions>
    pub fn api_version(&self) -> Version {
        self.info.api_version
    }

    /// Returns the device type.
    ///
    /// This may be used during device selection to choose a higher performance GPU.
    pub fn ty(&self) -> vk::PhysicalDeviceType {
        self.info.properties.device_type
    }

    /// Returns the Vulkan 1.0 physical device features.
    pub fn features(&self) -> vk::PhysicalDeviceFeatures {
        self.info.features
    }

    /// Returns the physical device properties.
    ///
    /// Some properties such as the device name can be obtained using other functions defined on
    /// [`PhysicalDevice`].
    pub fn properties(&self) -> vk::PhysicalDeviceProperties {
        self.info.properties
    }

    /// Returns the device's descriptor set properties.
    ///
    /// This also describes the maximum memory allocation size.
    pub fn properties_maintenance_3(&self) -> vk::PhysicalDeviceMaintenance3Properties<'_> {
        self.info.maintenance_3
    }

    /// Information about universally unique identifiers (UUIDs) that identify this device.
    pub fn id_properties(&self) -> vk::PhysicalDeviceIDProperties<'_> {
        self.info.id
    }

    /// Returns the physical device limits.
    pub fn limits(&self) -> vk::PhysicalDeviceLimits {
        self.info.properties.limits
    }

    /// Information about the Vulkan driver.
    ///
    /// This may return [`None`] for a few reasons:
    /// * The Vulkan implementation is not at least 1.2
    /// * If the Vulkan implementation is not at least Vulkan 1.2, the `VK_KHR_driver_properties` device
    ///   extension is not available.
    pub fn driver(&self) -> Option<&DriverInfo> {
        self.info.driver.as_ref()
    }

    /// Returns the major and minor numbers of the primary node which corresponds to this physical device's DRM
    /// device.
    #[cfg(feature = "backend_drm")]
    #[instrument(level = "debug", parent = &self.span, skip(self))]
    pub fn primary_node(&self) -> Result<Option<DrmNode>, UnsupportedProperty> {
        let properties_drm = self.info.get_drm_properties()?;
        let node = Some(properties_drm)
            .filter(|props| props.has_primary == vk::TRUE)
            .and_then(|props| {
                DrmNode::from_dev_id(libc::makedev(props.primary_major as _, props.primary_minor as _)).ok()
            });

        Ok(node)
    }

    /// Returns the major and minor numbers of the render node which corresponds to this physical device's DRM
    /// device.
    ///
    /// Note that not every device has a render node. If there is no render node (this function returns [`None`])
    /// then try to use the primary node.
    #[cfg(feature = "backend_drm")]
    #[instrument(level = "debug", parent = &self.span, skip(self))]
    pub fn render_node(&self) -> Result<Option<DrmNode>, UnsupportedProperty> {
        let properties_drm = self.info.get_drm_properties()?;
        let node = Some(properties_drm)
            .filter(|props| props.has_render == vk::TRUE)
            .and_then(|props| {
                DrmNode::from_dev_id(libc::makedev(props.render_major as _, props.render_minor as _)).ok()
            });

        Ok(node)
    }

    /// Get physical device properties.
    ///
    /// This function is equivalent to calling [`vkGetPhysicalDeviceProperties2`].
    ///
    /// # Safety
    ///
    /// - All valid usage requirements for [`vkGetPhysicalDeviceProperties2`] apply. Read the specification
    ///   for more information.
    ///
    /// [`vkGetPhysicalDeviceProperties2`]: https://www.khronos.org/registry/vulkan/specs/1.3-extensions/man/html/vkGetPhysicalDeviceProperties2.html
    pub unsafe fn get_properties(&self, props: &mut vk::PhysicalDeviceProperties2<'_>) {
        let instance = self.instance().handle();
        // SAFETY: The caller has guaranteed all valid usage requirements for vkGetPhysicalDeviceProperties2
        // are satisfied.
        unsafe { instance.get_physical_device_properties2(self.handle(), props) }
    }

    /// Get physical device format properties.
    ///
    /// This function is equivalent to calling [`vkGetPhysicalDeviceFormatProperties2`].
    ///
    /// # Safety
    ///
    /// - All valid usage requirements for [`vkGetPhysicalDeviceFormatProperties2`] apply. Read the specification
    ///   for more information.
    ///
    /// [`vkGetPhysicalDeviceFormatProperties2`]: https://www.khronos.org/registry/vulkan/specs/1.3-extensions/man/html/vkGetPhysicalDeviceFormatProperties2.html
    pub unsafe fn get_format_properties(&self, format: vk::Format, props: &mut vk::FormatProperties2<'_>) {
        let instance = self.instance().handle();
        // SAFETY: The caller has guaranteed all valid usage requirements for vkGetPhysicalDeviceFormatProperties2
        // are satisfied.
        unsafe { instance.get_physical_device_format_properties2(self.handle(), format, props) }
    }

    /// Returns properties for each supported DRM modifier for the specified format.
    ///
    /// Returns [`Err`] if the `VK_EXT_image_drm_format_modifier` extension is not supported.
    #[instrument(level = "debug", parent = &self.span, skip(self))]
    pub fn get_format_modifier_properties(
        &self,
        format: vk::Format,
    ) -> Result<Vec<vk::DrmFormatModifierPropertiesEXT>, UnsupportedProperty> {
        if !self.has_device_extension(ext::image_drm_format_modifier::NAME) {
            const EXTENSIONS: &[&CStr] = &[ext::image_drm_format_modifier::NAME];
            return Err(UnsupportedProperty::Extensions(EXTENSIONS));
        }

        // First get the number of modifiers the driver supports.
        let count = unsafe {
            let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
            let mut format_properties2 = vk::FormatProperties2::default().push_next(&mut list);
            self.get_format_properties(format, &mut format_properties2);
            list.drm_format_modifier_count as usize
        };

        // Allocate the vector to receive the modifiers in.
        let mut data = Vec::with_capacity(count);

        unsafe {
            let mut list = vk::DrmFormatModifierPropertiesListEXT {
                // We cannot use the builder here because the Vec is currently empty, so we need to tell Vulkan
                // where to write out the modifier properties and tell it how large the Vec is.
                p_drm_format_modifier_properties: data.as_mut_ptr(),
                drm_format_modifier_count: count as u32,
                ..Default::default()
            };

            let mut format_properties2 = vk::FormatProperties2::default().push_next(&mut list);
            self.get_format_properties(format, &mut format_properties2);
            // SAFETY: Vulkan just initialized the elements of the vector.
            data.set_len(list.drm_format_modifier_count as usize);
        }

        Ok(data)
    }

    /// Returns the device extensions supported by the physical device.
    pub fn device_extensions(&self) -> impl Iterator<Item = &CStr> {
        self.extensions.iter().map(CString::as_c_str)
    }

    /// Returns `true` if this device supports the specified device extension.
    pub fn has_device_extension(&self, extension: &CStr) -> bool {
        self.device_extensions().any(|name| name == extension)
    }

    /// Returns a handle to the underlying [`vk::PhysicalDevice`].
    ///
    /// The handle refers to a specific physical device advertised by the instance. This handle is only valid
    /// for the lifetime of the instance.
    pub fn handle(&self) -> vk::PhysicalDevice {
        self.phd
    }

    /// The instance which provided this physical device.
    pub fn instance(&self) -> &Instance {
        &self.instance
    }
}

impl PartialEq for PhysicalDevice {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // Both the physical device handle and instance handle must be the same
        self.phd == other.phd && self.instance().handle().handle() == other.instance().handle().handle()
    }
}

// SAFETY: The internal pointers in the PhysicalDevice*Properties are always null and only copies of the
// PhysicalDevice*Properties types are returned.
unsafe impl Send for PhysicalDevice {}
unsafe impl Sync for PhysicalDevice {}

/// Information about the driver providing a [`PhysicalDevice`].
#[derive(Debug, Clone)]
pub struct DriverInfo {
    /// ID which identifies the driver.
    pub id: vk::DriverId,

    /// The name of the driver.
    pub name: String,

    /// Information describing the driver.
    ///
    /// This may include information such as the driver version.
    pub info: String,

    /// The Vulkan conformance test this driver is conformant against.
    pub conformance: vk::ConformanceVersion,
}

#[derive(Debug, Clone, Default)]
struct PhdInfo {
    api_version: Version,
    name: String,
    properties: vk::PhysicalDeviceProperties,
    features: vk::PhysicalDeviceFeatures,
    maintenance_3: vk::PhysicalDeviceMaintenance3Properties<'static>,
    id: vk::PhysicalDeviceIDProperties<'static>,
    properties_driver: Option<PhysicalDeviceDriverProperties<'static>>,
    /// Information about the DRM device which corresponds to this physical device.
    #[cfg_attr(not(feature = "backend_drm"), allow(dead_code))]
    properties_drm: Option<PhysicalDeviceDrmPropertiesEXT<'static>>,
    driver: Option<DriverInfo>,
}

impl PhdInfo {
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
            // SAFETY: The caller has guaranteed the physical device supports VK_EXT_physical_device_drm
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

impl DriverInfo {
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
