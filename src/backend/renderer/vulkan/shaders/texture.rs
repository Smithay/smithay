use ash::vk::{DescriptorPoolSize, DescriptorSetLayoutBinding, DescriptorType, ShaderStageFlags};
use bytemuck::NoUninit;
use include_bytes_aligned::include_bytes_aligned;
use std::sync::LazyLock;

use crate::utils::{Buffer, Physical, Rectangle};

pub const TEX_SHADER: &[u8] = include_bytes_aligned!(32, concat!(env!("OUT_DIR"), "/vk/texture.glsl"));
pub static TEX_BINDINGS: LazyLock<[DescriptorSetLayoutBinding<'static>; 2]> = LazyLock::new(|| {
    [
        DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(DescriptorType::STORAGE_IMAGE)
            .stage_flags(ShaderStageFlags::COMPUTE)
            .descriptor_count(1),
        DescriptorSetLayoutBinding::default()
            .binding(1)
            .descriptor_type(DescriptorType::COMBINED_IMAGE_SAMPLER)
            .stage_flags(ShaderStageFlags::COMPUTE)
            .descriptor_count(1),
    ]
});
pub static TEX_SIZES: LazyLock<[DescriptorPoolSize; 2]> = LazyLock::new(|| {
    [
        DescriptorPoolSize::default().ty(DescriptorType::STORAGE_IMAGE),
        DescriptorPoolSize::default().ty(DescriptorType::COMBINED_IMAGE_SAMPLER),
    ]
});

#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, NoUninit)]
pub struct TexPushConstants {
    pub src_rect: Rectangle<f32, Buffer>,
    pub dst_rect: Rectangle<f32, Physical>,
    pub src_transform: u32,
    pub alpha: f32,
    pub damage_size: u32,
    pub _padding0: [u32; 1],
    pub damage: [Rectangle<i32, Physical>; 4],
}
