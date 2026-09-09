use ash::vk::{DescriptorPoolSize, DescriptorSetLayoutBinding, DescriptorType, ShaderStageFlags};
use bytemuck::NoUninit;
use include_bytes_aligned::include_bytes_aligned;
use std::sync::LazyLock;

use crate::utils::{Physical, Rectangle};

pub const CLEAR_SHADER: &[u8] = include_bytes_aligned!(32, concat!(env!("OUT_DIR"), "/vk/clear.glsl"));
pub static CLEAR_BINDINGS: LazyLock<[DescriptorSetLayoutBinding<'static>; 1]> = LazyLock::new(|| {
    [DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(DescriptorType::STORAGE_IMAGE)
        .stage_flags(ShaderStageFlags::COMPUTE)
        .descriptor_count(1)]
});
pub static CLEAR_SIZES: LazyLock<[DescriptorPoolSize; 1]> =
    LazyLock::new(|| [DescriptorPoolSize::default().ty(DescriptorType::STORAGE_IMAGE)]);

#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, NoUninit)]
pub struct ClearPushConstants {
    pub color: [f32; 4],
    pub blend: u32,
    pub size: u32,
    pub _padding0: [u32; 2],
    pub rects: [Rectangle<i32, Physical>; 6],
}
