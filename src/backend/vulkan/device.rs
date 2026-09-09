use crate::backend::{
    renderer::ContextId,
    vulkan::{format::FormatEntry, image::VulkanImage, PhysicalDevice},
};
use ash::{
    ext, khr,
    vk::{
        self, DeviceCreateInfo, DeviceQueueCreateInfo, PhysicalDeviceFeatures2,
        PhysicalDeviceMemoryProperties, Queue, QueueFlags,
    },
    Device as VkDevice,
};
use drm::node::DrmNode;
use std::{
    ffi::CStr,
    fmt,
    sync::{Arc, Weak},
};

#[derive(Debug, Clone)]
pub struct Device(Arc<InnerDevice>);
#[derive(Debug, Clone)]
pub struct WeakDevice(Weak<InnerDevice>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueueType {
    Transfer,
    Compute,
    Graphics,
}

impl QueueType {
    pub fn vk_flags(&self) -> QueueFlags {
        match self {
            QueueType::Transfer => QueueFlags::TRANSFER,
            QueueType::Compute => QueueFlags::COMPUTE,
            QueueType::Graphics => QueueFlags::GRAPHICS,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("No matching queue")]
    NoUsableQueue,
    /// Vulkan API error.
    #[error(transparent)]
    Vk(#[from] vk::Result),
}

impl Device {
    pub fn new(
        phd: &PhysicalDevice,
        extensions: &[&CStr],
        required_features: &mut PhysicalDeviceFeatures2<'_>,
        queue_type: QueueType,
        fallback_to_graphics: bool,
    ) -> Result<Self, DeviceError> {
        let extension_pointers = extensions.iter().copied().map(CStr::as_ptr).collect::<Vec<_>>();
        for ptr in &extension_pointers {
            println!("{:x} {:?}", (*ptr) as usize, unsafe { CStr::from_ptr(*ptr) });
        }

        let queue_families = unsafe {
            phd.instance()
                .handle()
                .get_physical_device_queue_family_properties(phd.handle())
        };
        let flags = queue_type.vk_flags();
        let queue_index = queue_families
            .iter()
            // Find a queue with a matching type
            .position(|properties| properties.queue_flags.contains(flags))
            // Fallback
            .or_else(|| {
                if fallback_to_graphics {
                    queue_families
                        .iter()
                        .position(|properties| properties.queue_flags.contains(QueueFlags::GRAPHICS))
                } else {
                    None
                }
            })
            .ok_or(DeviceError::NoUsableQueue)?;

        let mem_properties = unsafe {
            phd.instance()
                .handle()
                .get_physical_device_memory_properties(phd.handle())
        };

        let queue_info = DeviceQueueCreateInfo::default()
            .queue_family_index(queue_index as u32)
            .queue_priorities(&[0.0]);

        let queue_create_infos: &[DeviceQueueCreateInfo<'_>] = &[queue_info];

        let device_info = DeviceCreateInfo::default()
            .queue_create_infos(queue_create_infos)
            .enabled_extension_names(&extension_pointers)
            .push_next(required_features);

        let device = unsafe {
            phd.instance()
                .handle()
                .create_device(phd.handle(), &device_info, None)
                .map_err(DeviceError::Vk)?
        };

        let queue = unsafe { device.get_device_queue(queue_index as u32, 0) };

        let khr_external_semaphore_fd = if extensions
            .iter()
            .any(|ext| ext == &khr::external_semaphore_fd::NAME)
        {
            Some(khr::external_semaphore_fd::Device::new(
                phd.instance().handle(),
                &device,
            ))
        } else {
            None
        };
        let khr_external_memory_fd = if extensions.iter().any(|ext| ext == &khr::external_memory_fd::NAME) {
            Some(khr::external_memory_fd::Device::new(
                phd.instance().handle(),
                &device,
            ))
        } else {
            None
        };
        let ext_image_drm_format_modifier = if extensions
            .iter()
            .any(|ext| ext == &ext::image_drm_format_modifier::NAME)
        {
            Some(ext::image_drm_format_modifier::Device::new(
                phd.instance().handle(),
                &device,
            ))
        } else {
            None
        };
        let ext_host_image_copy = if extensions.iter().any(|ext| ext == &ext::host_image_copy::NAME) {
            Some(ext::host_image_copy::Device::new(
                phd.instance().handle(),
                &device,
            ))
        } else {
            None
        };

        Ok(Device(Arc::new(InnerDevice {
            vk: device,
            khr_external_semaphore_fd,
            khr_external_memory_fd,
            ext_image_drm_format_modifier,
            ext_host_image_copy,

            mem_properties,
            formats: phd.drm_formats(),
            node: phd
                .render_node()
                .ok()
                .flatten()
                .or_else(|| phd.primary_node().ok().flatten()),

            queue,
            queue_idx: queue_index as u32,

            context: ContextId::new(),
        })))
    }

    pub fn vk(&self) -> &VkDevice {
        &self.0.vk
    }

    pub fn vk_khr_external_semaphore_fd(&self) -> Option<&khr::external_semaphore_fd::Device> {
        self.0.khr_external_semaphore_fd.as_ref()
    }

    pub fn vk_khr_external_memory_fd(&self) -> Option<&khr::external_memory_fd::Device> {
        self.0.khr_external_memory_fd.as_ref()
    }

    pub fn vk_ext_image_drm_format_modifier(&self) -> Option<&ext::image_drm_format_modifier::Device> {
        self.0.ext_image_drm_format_modifier.as_ref()
    }

    pub fn vk_ext_host_image_copy(&self) -> Option<&ext::host_image_copy::Device> {
        self.0.ext_host_image_copy.as_ref()
    }

    pub fn memory_properties(&self) -> &PhysicalDeviceMemoryProperties {
        &self.0.mem_properties
    }

    pub fn queue(&self) -> &Queue {
        &self.0.queue
    }

    pub fn queue_family_idx(&self) -> u32 {
        self.0.queue_idx
    }

    pub fn formats(&self) -> impl Iterator<Item = &FormatEntry> {
        self.0.formats.iter()
    }

    pub fn node(&self) -> Option<DrmNode> {
        self.0.node.clone()
    }

    pub fn downgrade(&self) -> WeakDevice {
        WeakDevice(Arc::downgrade(&self.0))
    }

    pub fn context(&self) -> ContextId<VulkanImage> {
        self.0.context.clone()
    }
}

impl WeakDevice {
    pub fn upgrade(&self) -> Option<Device> {
        self.0.upgrade().map(Device)
    }
}

struct InnerDevice {
    vk: VkDevice,
    khr_external_semaphore_fd: Option<khr::external_semaphore_fd::Device>,
    khr_external_memory_fd: Option<khr::external_memory_fd::Device>,
    ext_image_drm_format_modifier: Option<ext::image_drm_format_modifier::Device>,
    ext_host_image_copy: Option<ext::host_image_copy::Device>,

    mem_properties: PhysicalDeviceMemoryProperties,
    formats: super::FormatList,
    #[cfg(feature = "backend_drm")]
    node: Option<DrmNode>,

    queue: Queue,
    queue_idx: u32,

    context: ContextId<VulkanImage>,
}

impl fmt::Debug for InnerDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VkDevice").finish_non_exhaustive()
    }
}

impl Drop for InnerDevice {
    fn drop(&mut self) {
        unsafe {
            self.vk.destroy_device(None);
        }
    }
}
