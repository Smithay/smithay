use ash::vk::{
    self, DescriptorPool, DescriptorPoolCreateFlags, DescriptorPoolCreateInfo,
    DescriptorSet as VkDescriptorSet, DescriptorSetAllocateInfo, DescriptorSetLayout, Result as VkError,
};

use std::sync::{Arc, Weak};

use crate::backend::vulkan::{device::WeakDevice, Device};

#[derive(Debug)]
pub struct DescriptorSet {
    device: WeakDevice,
    pool: Weak<DescriptorPool>,
    vk: VkDescriptorSet,
}

impl DescriptorSet {
    pub fn vk(&self) -> VkDescriptorSet {
        self.vk.clone()
    }
}

impl Drop for DescriptorSet {
    fn drop(&mut self) {
        if let Some((device, pool)) = self.device.upgrade().zip(self.pool.upgrade()) {
            unsafe {
                let _ = device.vk().free_descriptor_sets(*pool, &[self.vk]);
            }
        }
    }
}

#[derive(Debug)]
struct Pool {
    device: WeakDevice,
    vk: Option<Arc<DescriptorPool>>,
    len: usize,
    capacity: usize,
}

impl Drop for Pool {
    fn drop(&mut self) {
        if let Some((device, pool)) = self
            .device
            .upgrade()
            .zip(self.vk.take().and_then(Arc::into_inner))
        {
            unsafe {
                device.vk().destroy_descriptor_pool(pool, None);
            }
        }
    }
}

const START_DESCRIPTOR_COUNT: u32 = 256;

#[derive(Debug)]
pub struct DescriptorAllocator {
    device: WeakDevice,

    layout: DescriptorSetLayout,
    sizes: &'static [vk::DescriptorPoolSize],

    pools: Vec<Pool>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Device was destroyed")]
    LostDevice,
    #[error("Failed to allocate descriptor set")]
    AllocError(#[source] vk::Result),
    #[error("Failed to create descriptor pool")]
    DescriptorPool(#[source] vk::Result),
}

impl DescriptorAllocator {
    pub fn new(
        device: &Device,
        layout: DescriptorSetLayout,
        sizes: &'static [vk::DescriptorPoolSize],
    ) -> Self {
        DescriptorAllocator {
            device: device.downgrade(),
            layout,
            sizes,
            pools: Vec::new(),
        }
    }

    pub fn alloc_descriptor_set(&mut self) -> Result<DescriptorSet, Error> {
        let Some(device) = self.device.upgrade() else {
            return Err(Error::LostDevice);
        };

        let layouts = &[self.layout];
        let mut alloc_info = DescriptorSetAllocateInfo::default().set_layouts(layouts);

        for pool in self.pools.iter_mut() {
            if pool.len < pool.capacity {
                alloc_info = alloc_info.descriptor_pool((**pool.vk.as_ref().unwrap()).clone());
                match unsafe { device.vk().allocate_descriptor_sets(&alloc_info) } {
                    Err(VkError::ERROR_FRAGMENTED_POOL) | Err(VkError::ERROR_OUT_OF_POOL_MEMORY) => continue,
                    Ok(set) => {
                        pool.len += 1;
                        return Ok(DescriptorSet {
                            device: self.device.clone(),
                            pool: pool.vk.as_ref().map(Arc::downgrade).unwrap(),
                            vk: set[0],
                        });
                    }
                    Err(err) => return Err(Error::AllocError(err)),
                }
            }
        }

        // no (free) pool found
        let size = self
            .pools
            .last()
            .map(|pool| (pool.capacity * 2) as u32)
            .unwrap_or(START_DESCRIPTOR_COUNT);
        let mut sizes = Vec::from_iter(self.sizes.iter().copied());
        for pool_size in &mut sizes {
            *pool_size = pool_size.descriptor_count(size);
        }
        let create_info = DescriptorPoolCreateInfo::default()
            .pool_sizes(&sizes)
            .flags(DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
            .max_sets(size);
        let pool = unsafe {
            device
                .vk()
                .create_descriptor_pool(&create_info, None)
                .map_err(Error::DescriptorPool)?
        };
        self.pools.push(Pool {
            device: self.device.clone(),
            vk: Some(Arc::new(pool)),
            len: 0,
            capacity: size as usize,
        });

        let pool = self.pools.last_mut().unwrap();
        alloc_info = alloc_info.descriptor_pool((**pool.vk.as_ref().unwrap()).clone());
        let set = unsafe {
            device
                .vk()
                .allocate_descriptor_sets(&alloc_info)
                .map_err(Error::AllocError)?
        };
        pool.len += 1;
        Ok(DescriptorSet {
            device: self.device.clone(),
            pool: pool.vk.as_ref().map(Arc::downgrade).unwrap(),
            vk: set[0],
        })
    }
}

impl Drop for DescriptorAllocator {
    fn drop(&mut self) {
        if let Some(device) = self.device.upgrade() {
            unsafe {
                device.vk().destroy_descriptor_set_layout(self.layout, None);
            }
        }
    }
}
