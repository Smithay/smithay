use super::Error;
use crate::backend::{
    renderer::vulkan::shaders::DescriptorSet,
    vulkan::device::{Device, WeakDevice},
};

use ash::vk::{
    CommandBuffer, CommandBufferAllocateInfo, CommandBufferBeginInfo, CommandBufferLevel,
    CommandBufferUsageFlags, CommandPool as VkCommandPool,
};
use std::collections::VecDeque;

#[derive(Debug)]
pub struct CommandPool {
    device: WeakDevice,
    vk: VkCommandPool,
    pending_buffers: VecDeque<(u64, CommandBuffer, Option<DescriptorSet>)>,
}

impl CommandPool {
    pub(super) fn from_vk(device: &Device, vk: VkCommandPool) -> Self {
        CommandPool {
            device: device.downgrade(),
            vk,
            pending_buffers: VecDeque::new(),
        }
    }

    pub fn create_and_begin_buffer(&self) -> Result<CommandBuffer, Error> {
        let Some(device) = self.device.upgrade() else {
            return Err(Error::DeadDevice);
        };

        let allocate_info = CommandBufferAllocateInfo::default()
            .command_pool(self.vk.clone())
            .level(CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);

        let buf = unsafe {
            device
                .vk()
                .allocate_command_buffers(&allocate_info)
                .map_err(Error::CommandBufferError)?[0]
        };

        let begin_info = CommandBufferBeginInfo::default().flags(CommandBufferUsageFlags::ONE_TIME_SUBMIT);

        unsafe {
            device
                .vk()
                .begin_command_buffer(buf.clone(), &begin_info)
                .map_err(Error::CommandBufferError)?;
        }

        Ok(buf)
    }

    pub fn store_pending_buffer(
        &mut self,
        buf: CommandBuffer,
        seq: u64,
        desc: impl Into<Option<DescriptorSet>>,
    ) {
        self.pending_buffers.push_back((seq, buf, desc.into()));
    }

    pub fn clean_old_buffers(&mut self, seq: u64) {
        let idx = self.pending_buffers.iter().position(|(s, _, _)| *s > seq);

        // TODO: truncate_front when stable
        let bufs = (if let Some(idx) = idx {
            self.pending_buffers.drain(..idx)
        } else {
            self.pending_buffers.drain(..)
        })
        .map(|(_, buf, _)| buf)
        .collect::<Vec<CommandBuffer>>();

        if let Some(device) = self.device.upgrade() {
            if !bufs.is_empty() {
                unsafe {
                    device.vk().free_command_buffers(self.vk.clone(), &bufs);
                }
            }
        }
    }
}

impl Drop for CommandPool {
    fn drop(&mut self) {
        if let Some(device) = self.device.upgrade() {
            let bufs = self
                .pending_buffers
                .drain(..)
                .map(|(_, buf, _)| buf)
                .collect::<Vec<_>>();

            unsafe {
                let _ = device.vk().device_wait_idle();
                if !bufs.is_empty() {
                    device.vk().free_command_buffers(self.vk, &bufs);
                }
                device.vk().destroy_command_pool(self.vk.clone(), None);
            }
        }
    }
}
