use ash::vk::{
    self, BufferCreateInfo, BufferUsageFlags, CommandBuffer, MemoryAllocateInfo, MemoryPropertyFlags,
    PhysicalDeviceMemoryProperties, SharingMode,
};
use bytemuck::Pod;
use std::{ffi::c_void, marker::PhantomData, mem};

use crate::backend::vulkan::{device::WeakDevice, Device};

pub struct VecBuffer<T: Pod> {
    device: WeakDevice,
    len: usize,
    capacity: usize,

    buf: vk::Buffer,
    memory: vk::DeviceMemory,
    map: *const c_void,

    _elem: std::marker::PhantomData<T>,
}

const DEFAULT_CAPACITY: usize = 256;

// Device needs to have one DEVICE_LOCAL
// Device needs to have one HOST_VISIBLE and HOST_COHERENT
// Try all three and then fallback to transfer
// Pod + repr of Rectangle/Point/Size

fn mem_idx(mem_props: &PhysicalDeviceMemoryProperties, flags: MemoryPropertyFlags) -> Option<u32> {
    mem_props
        .memory_types_as_slice()
        .iter()
        .enumerate()
        .find_map(|(idx, type_)| {
            if (type_.property_flags & flags) == flags {
                Some(idx as u32)
            } else {
                None
            }
        })
}

impl<T: Pod> VecBuffer<T> {
    pub fn new(device: &Device, usage: BufferUsageFlags) -> Result<VecBuffer<T>> {
        Self::with_capacity(device, DEFAULT_CAPACITY, usage)
    }

    pub fn with_capacity(device: &Device, cap: usize, usage: BufferUsageFlags) -> Result<VecBuffer<T>> {
        let create_info = BufferCreateInfo::default()
            .usage(usage | BufferUsageFlags::TRANSFER_DST)
            .size(cap as u64)
            .sharing_mode(SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.vk().create_buffer(&create_info, None)? };
        let mem_req = unsafe { device.vk().get_buffer_memory_requirements(buffer.clone()) };
        let (needs_staging, mem_idx) = mem_idx(
            device.memory_properties(),
            MemoryPropertyFlags::DEVICE_LOCAL
                | MemoryPropertyFlags::HOST_VISIBLE
                | MemoryPropertyFlags::HOST_COHERENT,
        )
        .map(|idx| (false, idx))
        .unwrap_or_else(|| {
            (
                true,
                mem_idx(device.memory_properties(), MemoryPropertyFlags::DEVICE_LOCAL)
                    .expect("No device local memory"),
            )
        });

        let alloc_info = MemoryAllocateInfo::default()
            .allocation_size(mem_req.size)
            .memory_type_index(mem_idx as u32);
        let memory = unsafe { device.vk().allocate_memory(&alloc_info, None)? };
        unsafe {
            device.vk().bind_buffer_memory(buffer, memory, 0)?;
        }

        let (upload_buf, upload_mem) = if needs_staging {
            let mem_idx = mem_idx(
                device.memory_properties(),
                MemoryPropertyFlags::HOST_VISIBLE | MemoryPropertyFlags::HOST_COHERENT,
            )
            .expect("No host memory");
            let create_info = BufferCreateInfo::default()
                .usage(BufferUsageFlags::TRANSFER_SRC | BufferUsageFlags::TRANSFER_DST)
                .size(DEFAULT_CAPACITY as u64)
                .sharing_mode(SharingMode::EXCLUSIVE);
            let buffer = unsafe { device.vk().create_buffer(&create_info, None)? };
            let mem_req = unsafe { device.vk().get_buffer_memory_requirements(buffer.clone()) };

            let alloc_info = MemoryAllocateInfo::default()
                .allocation_size(mem_req.size)
                .memory_type_index(mem_idx as u32);
            let memory = unsafe { device.vk().allocate_memory(&alloc_info, None)? };
            unsafe {
                device.vk().bind_buffer_memory(buffer, memory, 0)?;
            }

            (Some(buffer), Some(memory))
        } else {
            (None, None)
        };

        Ok(VecBuffer {
            device: device.downgrade(),
            len: 0,
            capacity: cap,

            buf: buffer,
            memory,

            upload_buf,
            upload_mem,

            _elem: PhantomData,
        })
    }

    pub fn from_iter(
        device: &Device,
        usage: BufferUsageFlags,
        iter: impl Iterator<Item = T>,
    ) -> Result<VecBuffer<T>> {
        let (lower, upper) = iter.size_hint();
        let capacity = upper.unwrap_or(lower) * std::mem::size_of::<T>();
        let mut buf = Self::with_capacity(device, capacity, usage)?;
        buf.extend(iter);
        Ok(buf)
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn map<'a>(&'a self) -> Result<Option<BufReadGuard<'a>>> {}

    pub fn map_mut<'a>(&'a mut self) -> Result<Option<BufWriteGuard<'a>>> {}
}

impl<T: Pod> Extend for VecBuffer<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {}
}

impl Drop for VecBuffer {
    fn drop(&mut self) {
        if let Some(device) = self.device.upgrade() {
            unsafe {
                device.vk().destroy_buffer(self.buf, None);
                device.vk().free_memory(self.memory, None);

                if let Some((buf, mem)) = self.upload_buf.zip(self.upload_mem) {
                    device.vk().destroy_buffer(buf, None);
                    device.vk().free_memory(mem, None);
                }
            }
        }
    }
}
