use std::os::fd::{FromRawFd, OwnedFd};

use crate::backend::{
    drm::{sync::DrmTimeline, DrmDeviceFd},
    renderer::vulkan::{cmds::CommandPool, sync::VulkanTimeline, Error},
    vulkan::Device,
};
use ash::{
    khr,
    vk::{
        CommandPoolCreateInfo, ExportSemaphoreCreateInfo, ExternalSemaphoreHandleTypeFlags,
        SemaphoreCreateInfo, SemaphoreGetFdInfoKHR, SemaphoreType, SemaphoreTypeCreateInfo,
    },
};

impl Device {
    pub(super) fn create_timeline_semaphore(
        &self,
        export: Option<DrmDeviceFd>,
    ) -> Result<VulkanTimeline, Error> {
        let mut semaphore_type_info = SemaphoreTypeCreateInfo::default()
            .semaphore_type(SemaphoreType::TIMELINE)
            .initial_value(0);
        let mut semaphore_export_info =
            ExportSemaphoreCreateInfo::default().handle_types(ExternalSemaphoreHandleTypeFlags::OPAQUE_FD);

        let mut semaphore_create_info = SemaphoreCreateInfo::default().push_next(&mut semaphore_type_info);
        if export.is_some() {
            semaphore_create_info = semaphore_create_info.push_next(&mut semaphore_export_info);
        }

        let semaphore = unsafe {
            self.vk()
                .create_semaphore(&semaphore_create_info, None)
                .map_err(Error::SemaphoreError)?
        };

        let drm = if let Some(dev) = export {
            let Some(khr_external_semaphore_fd) = self.vk_khr_external_semaphore_fd() else {
                return Err(Error::MissingExtension(khr::external_semaphore_fd::NAME));
            };

            let semaphore_get_info = SemaphoreGetFdInfoKHR::default()
                .semaphore(semaphore.clone())
                .handle_type(ExternalSemaphoreHandleTypeFlags::OPAQUE_FD);

            let fd = unsafe {
                OwnedFd::from_raw_fd(
                    khr_external_semaphore_fd
                        .get_semaphore_fd(&semaphore_get_info)
                        .map_err(Error::SemaphoreError)?,
                )
            };
            DrmTimeline::new(&dev, fd).ok()
        } else {
            None
        };

        Ok(VulkanTimeline {
            device: self.downgrade(),
            vk: semaphore,
            drm,
        })
    }

    pub(super) fn create_command_pool(&self) -> Result<CommandPool, Error> {
        let pool_info = CommandPoolCreateInfo::default().queue_family_index(self.queue_family_idx());
        let cmd_pool = unsafe {
            self.vk()
                .create_command_pool(&pool_info, None)
                .map_err(Error::CommandPoolError)?
        };

        Ok(CommandPool::from_vk(self, cmd_pool))
    }
}
