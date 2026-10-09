use ash::{
    ext,
    vk::{
        self, BufferCreateInfo, BufferUsageFlags, ExternalMemoryHandleTypeFlags,
        ImportMemoryHostPointerInfoEXT, MemoryAllocateInfo, MemoryHostPointerPropertiesEXT, MemoryMapFlags,
        MemoryPropertyFlags, PhysicalDeviceMemoryProperties, SharingMode,
    },
};
use std::{
    ffi::{c_void, CStr},
    ptr::NonNull,
};

use crate::backend::vulkan::{device::WeakDevice, Device};

pub struct Buffer {
    device: WeakDevice,

    buf: vk::Buffer,
    memory: vk::DeviceMemory,
    map: Option<NonNull<c_void>>,

    device_local: bool,
    size: usize,
}

// Device needs to have one DEVICE_LOCAL
// Device needs to have one HOST_VISIBLE and HOST_COHERENT

fn mem_idx(mem_props: &PhysicalDeviceMemoryProperties, mask: u32, flags: MemoryPropertyFlags) -> Option<u32> {
    mem_props
        .memory_types_as_slice()
        .iter()
        .enumerate()
        .find_map(|(idx, type_)| {
            let idx_mask = 1 << idx;
            if (mask & idx_mask as u32) != 0 && (type_.property_flags & flags) == flags {
                Some(idx as u32)
            } else {
                None
            }
        })
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Failed to find a memory type satisfying all constraints")]
    NoMatchingMemoryType,
    #[error("Failed to create buffer")]
    BufferCreation(#[source] vk::Result),
    #[error("Failed to allocate device memory")]
    Allocation(#[source] vk::Result),
    #[error("Failed to bind buffer to device memory")]
    BindingBuffer(#[source] vk::Result),
    #[error("Missing extension for buffer creation: {0:?}")]
    MissingExtension(&'static CStr),
    #[error("Invalid pointer alignment for operation")]
    InvalidAlignment,
    #[error("Failed to query host pointer properties")]
    HostPtrQuery(#[source] vk::Result),
    #[error("Failed to map device memory")]
    Map(#[source] vk::Result),
}

impl Buffer {
    pub fn from_ptr(
        device: &Device,
        data: *mut u8,
        len: usize,
        handle: ExternalMemoryHandleTypeFlags,
        usage: BufferUsageFlags,
    ) -> Result<Buffer, Error> {
        let Some((device_external_memory_host, min_align)) = device.vk_ext_external_memory_host() else {
            return Err(Error::MissingExtension(ext::external_memory_host::NAME));
        };

        if data.addr() as u64 % *min_align != 0 || len as u64 % *min_align != 0 {
            return Err(Error::InvalidAlignment);
        }

        // TODO: maybe use jay-ash?
        let mut props = MemoryHostPointerPropertiesEXT::default();
        let res = unsafe {
            (device_external_memory_host
                .fp()
                .get_memory_host_pointer_properties_ext)(
                device.vk().handle(),
                ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT,
                data as *const _,
                &mut props as *mut _,
            )
        };
        res.result().map_err(Error::HostPtrQuery)?;

        let create_info = BufferCreateInfo::default()
            .usage(usage)
            .size(len as u64)
            .sharing_mode(SharingMode::EXCLUSIVE);

        let buffer = unsafe {
            device
                .vk()
                .create_buffer(&create_info, None)
                .map_err(Error::BufferCreation)?
        };
        let mem_req = unsafe { device.vk().get_buffer_memory_requirements(buffer.clone()) };
        let mem_mask = props.memory_type_bits & mem_req.memory_type_bits;
        let mem_idx = mem_idx(device.memory_properties(), mem_mask, MemoryPropertyFlags::empty())
            .ok_or(Error::NoMatchingMemoryType)?;

        let mut ptr_info = ImportMemoryHostPointerInfoEXT::default()
            .handle_type(handle)
            .host_pointer(data as *mut _);
        let alloc_info = MemoryAllocateInfo::default()
            .allocation_size(mem_req.size)
            .memory_type_index(mem_idx as u32)
            .push_next(&mut ptr_info);
        let memory = unsafe {
            device
                .vk()
                .allocate_memory(&alloc_info, None)
                .map_err(Error::Allocation)?
        };
        unsafe {
            device
                .vk()
                .bind_buffer_memory(buffer, memory, 0)
                .map_err(Error::BindingBuffer)?;
        }

        Ok(Buffer {
            device: device.downgrade(),

            buf: buffer,
            memory,
            map: None,

            device_local: false,
            size: len,
        })
    }

    pub fn new(
        device: &Device,
        size: usize,
        mem_mask: Option<u32>,
        usage: BufferUsageFlags,
        mappable: bool,
    ) -> Result<Buffer, Error> {
        let create_info = BufferCreateInfo::default()
            .usage(usage)
            .size(size as u64)
            .sharing_mode(SharingMode::EXCLUSIVE);

        let buffer = unsafe {
            device
                .vk()
                .create_buffer(&create_info, None)
                .map_err(Error::BufferCreation)?
        };
        let mem_req = unsafe { device.vk().get_buffer_memory_requirements(buffer.clone()) };
        let mem_mask = mem_mask.unwrap_or(u32::MAX) & mem_req.memory_type_bits;
        let (device_local, mem_idx) = if mappable {
            mem_idx(
                device.memory_properties(),
                mem_mask,
                MemoryPropertyFlags::DEVICE_LOCAL
                    | MemoryPropertyFlags::HOST_VISIBLE
                    | MemoryPropertyFlags::HOST_COHERENT,
            )
            .map(|idx| (true, idx))
            .or_else(|| {
                mem_idx(
                    device.memory_properties(),
                    mem_mask,
                    MemoryPropertyFlags::HOST_VISIBLE | MemoryPropertyFlags::HOST_COHERENT,
                )
                .map(|idx| (false, idx))
            })
            .ok_or(Error::NoMatchingMemoryType)?
        } else {
            mem_idx(
                device.memory_properties(),
                mem_mask,
                MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .map(|idx| (true, idx))
            .or_else(|| {
                mem_idx(device.memory_properties(), mem_mask, MemoryPropertyFlags::empty())
                    .map(|idx| (false, idx))
            })
            .ok_or(Error::NoMatchingMemoryType)?
        };

        let alloc_info = MemoryAllocateInfo::default()
            .allocation_size(mem_req.size)
            .memory_type_index(mem_idx as u32);
        let memory = unsafe {
            device
                .vk()
                .allocate_memory(&alloc_info, None)
                .map_err(Error::Allocation)?
        };
        unsafe {
            device
                .vk()
                .bind_buffer_memory(buffer, memory, 0)
                .map_err(Error::BindingBuffer)?;
        }

        let map = if mappable {
            unsafe {
                NonNull::new(
                    device
                        .vk()
                        .map_memory(memory, 0, mem_req.size, MemoryMapFlags::empty())
                        .map_err(Error::Map)?,
                )
            }
        } else {
            None
        };

        Ok(Buffer {
            device: device.downgrade(),

            buf: buffer,
            memory,
            map,

            device_local,
            size,
        })
    }

    pub fn vk(&self) -> &vk::Buffer {
        &self.buf
    }

    pub fn mapped_ptr(&self) -> Option<NonNull<c_void>> {
        self.map
    }

    pub fn is_device_local(&self) -> bool {
        self.device_local
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        if let Some(device) = self.device.upgrade() {
            unsafe {
                device.vk().destroy_buffer(self.buf, None);
                if let Some(_) = self.map.take() {
                    device.vk().unmap_memory(self.memory);
                }
                device.vk().free_memory(self.memory, None);
            }
        }
    }
}
