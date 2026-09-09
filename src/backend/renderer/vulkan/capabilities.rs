use std::{
    ffi::{c_void, CStr},
    pin::Pin,
};

use ash::{
    ext, khr,
    vk::{
        ExternalSemaphoreFeatureFlags, ExternalSemaphoreHandleTypeFlags, ExternalSemaphoreProperties,
        PhysicalDeviceExternalSemaphoreInfo, PhysicalDeviceFeatures2, PhysicalDeviceHostImageCopyFeaturesEXT,
        PhysicalDeviceVulkan11Features, PhysicalDeviceVulkan12Features, PhysicalDeviceVulkan13Features,
        SemaphoreType, SemaphoreTypeCreateInfo,
    },
};

use crate::backend::vulkan::{version::Version, PhysicalDevice};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    DmabufMemory,
    HostImageCopy,
    ExportTimeline,
}

pub struct Features {
    features: PhysicalDeviceFeatures2<'static>,
    features_11: PhysicalDeviceVulkan11Features<'static>,
    features_12: PhysicalDeviceVulkan12Features<'static>,
    features_ext_host_image_copy: PhysicalDeviceHostImageCopyFeaturesEXT<'static>,
    // we require vulkan 1.2 at minimum
    features_13: Option<PhysicalDeviceVulkan13Features<'static>>,
    self_ref: std::marker::PhantomPinned,
}

impl Features {
    pub fn new(version: &Version) -> Pin<Box<Self>> {
        let mut features = Box::pin(Features {
            features: PhysicalDeviceFeatures2::default(),
            features_11: PhysicalDeviceVulkan11Features::default(),
            features_12: PhysicalDeviceVulkan12Features::default(),
            features_ext_host_image_copy: PhysicalDeviceHostImageCopyFeaturesEXT::default(),
            features_13: None,
            self_ref: std::marker::PhantomPinned,
        });

        {
            let features = unsafe { features.as_mut().get_unchecked_mut() };
            features.features.p_next = &mut features.features_11 as *mut _ as *mut c_void;
            features.features_11.p_next = &mut features.features_12 as *mut _ as *mut c_void;
            features.features_12.p_next = &mut features.features_ext_host_image_copy as *mut _ as *mut c_void;

            if version >= &Version::VERSION_1_3 {
                features.features_13 = Some(PhysicalDeviceVulkan13Features::default());
                features.features_ext_host_image_copy.p_next =
                    features.features_13.as_mut().unwrap() as *mut _ as *mut c_void;
            }
        }

        features
    }

    pub fn required_features() -> Pin<Box<Self>> {
        let mut features = Self::new(&Version::VERSION_1_2);

        {
            let features = unsafe { features.as_mut().get_unchecked_mut() };
            features.features_12.timeline_semaphore = 1;
            features.features_ext_host_image_copy.host_image_copy = 1;
        }

        features
    }

    pub fn has_required_features(&self) -> Result<(), &'static str> {
        // TODO: I'd love to have a more generic `satisfies(&self, other: &Self)` method,
        // but there isn't a nice way to iterate over fields of a struct and do the necessary bool comparision.
        // And I am not at the point yet of thinking to write a vulkan generator, na-uh!

        if self.features_12.timeline_semaphore == 0 {
            return Err("timeline_semaphore");
        }
        if self.features_ext_host_image_copy.host_image_copy == 0 {
            return Err("host_image_copy");
        }

        Ok(())
    }

    pub fn supported_features(phd: &PhysicalDevice) -> Pin<Box<Self>> {
        let mut features = Self::new(&phd.api_version());

        unsafe {
            phd.instance().handle().get_physical_device_features2(
                phd.handle(),
                &mut features.as_mut().get_unchecked_mut().features,
            );
        }

        features
    }

    // SAFETY: You must not move the struct
    pub unsafe fn vk(self: &mut Pin<Box<Self>>) -> &mut PhysicalDeviceFeatures2<'static> {
        &mut self.as_mut().get_unchecked_mut().features
    }
}

impl Capability {
    pub fn supports_export_timeline(phd: &PhysicalDevice) -> Option<Capability> {
        if !phd.has_device_extension(khr::external_semaphore_fd::NAME) {
            return None;
        }

        let mut create_info = SemaphoreTypeCreateInfo::default().semaphore_type(SemaphoreType::TIMELINE);
        let external_info = PhysicalDeviceExternalSemaphoreInfo::default()
            .handle_type(ExternalSemaphoreHandleTypeFlags::OPAQUE_FD)
            .push_next(&mut create_info);
        let mut semaphore_props = ExternalSemaphoreProperties::default();
        unsafe {
            phd.instance()
                .handle()
                .get_physical_device_external_semaphore_properties(
                    phd.handle(),
                    &external_info,
                    &mut semaphore_props,
                )
        };

        semaphore_props
            .external_semaphore_features
            .contains(ExternalSemaphoreFeatureFlags::IMPORTABLE | ExternalSemaphoreFeatureFlags::EXPORTABLE)
            .then_some(Capability::ExportTimeline)
    }

    pub fn supports_host_image_copy(phd: &PhysicalDevice) -> Option<Capability> {
        if !phd.has_device_extension(ext::host_image_copy::NAME) {
            return None;
        }

        Some(Capability::HostImageCopy)
    }

    pub fn supports_dmabuf_memory(phd: &PhysicalDevice) -> Option<Capability> {
        for ext in [
            khr::external_memory_fd::NAME,
            ext::image_drm_format_modifier::NAME,
            ext::external_memory_dma_buf::NAME,
        ] {
            if !phd.has_device_extension(ext) {
                return None;
            }
        }

        Some(Capability::DmabufMemory)
    }

    pub fn as_extensions(caps: &[Capability]) -> Vec<&CStr> {
        caps.iter()
            .flat_map(|cap| match cap {
                Capability::DmabufMemory => &[
                    khr::external_memory_fd::NAME,
                    ext::image_drm_format_modifier::NAME,
                    ext::external_memory_dma_buf::NAME,
                ] as &'static [&CStr],
                Capability::HostImageCopy => &[ext::host_image_copy::NAME],
                Capability::ExportTimeline => &[khr::external_semaphore_fd::NAME],
            })
            .copied()
            .collect()
    }
}
