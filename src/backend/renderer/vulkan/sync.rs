use crate::backend::{drm::sync::DrmTimeline, vulkan::device::WeakDevice};
use ash::vk::Semaphore;

#[derive(Debug)]
pub struct VulkanTimeline {
    pub(super) device: WeakDevice,
    pub(super) vk: Semaphore,
    pub(super) drm: Option<DrmTimeline>,
}

impl Drop for VulkanTimeline {
    fn drop(&mut self) {
        if let Some(device) = self.device.upgrade() {
            unsafe { device.vk().destroy_semaphore(self.vk.clone(), None) };
        }
    }
}
