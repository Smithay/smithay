use std::sync::Arc;

use crate::backend::{
    drm::sync::DrmTimeline,
    renderer::ContextId,
    vulkan::{device::WeakDevice, image::VulkanImage, Device},
};
use ash::vk::{Result as VkResult, Semaphore, SemaphoreWaitInfo};

#[derive(Debug, Clone)]
pub struct VulkanTimeline(Arc<VulkanTimelineInternal>);

#[derive(Debug)]
struct VulkanTimelineInternal {
    pub(super) device: WeakDevice,
    pub(super) vk: Semaphore,
    pub(super) drm: Option<DrmTimeline>,
}

impl VulkanTimeline {
    pub fn new(device: &Device, vk: Semaphore, drm: impl Into<Option<DrmTimeline>>) -> Self {
        VulkanTimeline(Arc::new(VulkanTimelineInternal {
            device: device.downgrade(),
            vk,
            drm: drm.into(),
        }))
    }

    pub fn vk(&self) -> Semaphore {
        self.0.vk
    }

    pub fn query_signalled_point(&self) -> Option<u64> {
        let Some(device) = self.0.device.upgrade() else {
            return None;
        };

        unsafe { device.vk().get_semaphore_counter_value(self.0.vk).ok() }
    }

    pub fn is_exportable(&self) -> bool {
        self.0.drm.is_some()
    }

    pub fn export(&self) -> Option<DrmTimeline> {
        self.0.drm.clone()
    }
}

impl Drop for VulkanTimelineInternal {
    fn drop(&mut self) {
        if let Some(device) = self.device.upgrade() {
            unsafe { device.vk().destroy_semaphore(self.vk.clone(), None) };
        }
    }
}

#[derive(Debug)]
pub struct VulkanSync {
    pub timeline: VulkanTimeline,
    pub seq: u64,
}

impl VulkanSync {
    pub fn ctx(&self) -> Option<ContextId<VulkanImage>> {
        let Some(device) = self.timeline.0.device.upgrade() else {
            return None;
        };

        Some(device.context())
    }

    pub fn wait(&self, timeout: u64) -> Result<(), VkResult> {
        let Some(device) = self.timeline.0.device.upgrade() else {
            return Err(VkResult::ERROR_DEVICE_LOST);
        };

        let semaphores = [self.timeline.0.vk];
        let values = [self.seq];
        unsafe {
            device.vk().wait_semaphores(
                &SemaphoreWaitInfo::default()
                    .semaphores(&semaphores)
                    .values(&values),
                timeout,
            )
        }
    }
}
