use ash::vk::{self, Pipeline, PipelineLayout, PipelineShaderStageCreateInfo, ShaderStageFlags};

use crate::backend::{
    renderer::vulkan::shaders::descriptor::DescriptorAllocator,
    vulkan::{device::WeakDevice, Device},
};

mod clear;
mod descriptor;
mod texture;
use self::clear::*;
pub use self::descriptor::DescriptorSet;
use self::texture::*;
pub use self::{clear::ClearPushConstants, texture::TexPushConstants};

pub fn spirv_u32(shader: &[u8]) -> &[u32] {
    let len = shader.len();
    assert!(len.is_multiple_of(4));
    bytemuck::cast_slice(shader)
}

#[derive(Debug)]
pub struct Pipelines {
    device: WeakDevice,
    cache: vk::PipelineCache,

    clear_pipeline: Pipeline,
    clear_layout: PipelineLayout,
    clear_desc_pool: DescriptorAllocator,
    tex_pipeline: Pipeline,
    tex_layout: PipelineLayout,
    tex_desc_pool: DescriptorAllocator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinShader {
    Clear,
    Texture,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Failed to create pipeline cache")]
    CacheCreation(#[source] vk::Result),
    #[error("Failed to create shader module")]
    Shader(#[source] vk::Result),
    #[error("Failed to create descriptor set layout")]
    DescriptorSetLayout(#[source] vk::Result),
    #[error(transparent)]
    DescriptorSet(#[from] self::descriptor::Error),
    #[error("Failed to create pipeline layout")]
    PipelineLayout(#[source] vk::Result),
    #[error("Failed to create pipelines")]
    Pipeline(#[source] vk::Result),
}

impl Pipelines {
    pub fn new(device: &Device) -> Result<Self, Error> {
        let create_info = vk::PipelineCacheCreateInfo::default();
        let cache = unsafe {
            device
                .vk()
                .create_pipeline_cache(&create_info, None)
                .map_err(Error::CacheCreation)?
        };

        let create_info = vk::ShaderModuleCreateInfo::default().code(spirv_u32(CLEAR_SHADER));
        let clear_shader = unsafe {
            device
                .vk()
                .create_shader_module(&create_info, None)
                .map_err(Error::Shader)?
        };

        let create_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(CLEAR_BINDINGS.as_slice());
        let descriptor_set = unsafe {
            device
                .vk()
                .create_descriptor_set_layout(&create_info, None)
                .map_err(Error::DescriptorSetLayout)?
        };
        let layouts = [descriptor_set];

        let constants = [vk::PushConstantRange::default()
            .stage_flags(ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(std::mem::size_of::<ClearPushConstants>() as u32)];
        let create_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&layouts)
            .push_constant_ranges(&constants);
        let clear_layout = unsafe {
            device
                .vk()
                .create_pipeline_layout(&create_info, None)
                .map_err(Error::PipelineLayout)?
        };
        let clear_desc_pool = DescriptorAllocator::new(&device, descriptor_set, &*CLEAR_SIZES);

        let create_info = vk::ShaderModuleCreateInfo::default().code(spirv_u32(TEX_SHADER));
        let tex_shader = unsafe {
            device
                .vk()
                .create_shader_module(&create_info, None)
                .map_err(Error::Shader)?
        };

        let create_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(TEX_BINDINGS.as_slice());
        let descriptor_set = unsafe {
            device
                .vk()
                .create_descriptor_set_layout(&create_info, None)
                .map_err(Error::DescriptorSetLayout)?
        };
        let layouts = [descriptor_set];

        let constants = [vk::PushConstantRange::default()
            .stage_flags(ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(std::mem::size_of::<TexPushConstants>() as u32)];
        let create_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&layouts)
            .push_constant_ranges(&constants);
        let tex_layout = unsafe {
            device
                .vk()
                .create_pipeline_layout(&create_info, None)
                .map_err(Error::PipelineLayout)?
        };
        let tex_desc_pool = DescriptorAllocator::new(&device, descriptor_set, &*TEX_SIZES);

        let create_infos = [
            vk::ComputePipelineCreateInfo::default()
                .stage(
                    PipelineShaderStageCreateInfo::default()
                        .stage(ShaderStageFlags::COMPUTE)
                        .name(c"main")
                        .module(clear_shader),
                )
                .layout(clear_layout.clone()),
            vk::ComputePipelineCreateInfo::default()
                .stage(
                    PipelineShaderStageCreateInfo::default()
                        .stage(ShaderStageFlags::COMPUTE)
                        .name(c"main")
                        .module(tex_shader),
                )
                .layout(tex_layout.clone()),
        ];
        let pipelines = unsafe {
            device
                .vk()
                .create_compute_pipelines(cache, &create_infos, None)
                .map_err(|(pipelines, res)| {
                    for pipeline in pipelines {
                        device.vk().destroy_pipeline(pipeline, None);
                    }
                    res
                })
                .map_err(Error::Pipeline)?
        };

        unsafe {
            device.vk().destroy_shader_module(clear_shader, None);
            device.vk().destroy_shader_module(tex_shader, None);
        }

        Ok(Pipelines {
            device: device.downgrade(),
            cache,

            clear_pipeline: pipelines[0],
            clear_layout,
            clear_desc_pool,
            tex_pipeline: pipelines[1],
            tex_layout,
            tex_desc_pool,
        })
    }

    pub fn clear_pipeline(&self) -> &Pipeline {
        &self.clear_pipeline
    }
    pub fn clear_pipeline_layout(&self) -> &PipelineLayout {
        &self.clear_layout
    }

    pub fn tex_pipeline(&self) -> &Pipeline {
        &self.tex_pipeline
    }
    pub fn tex_pipeline_layout(&self) -> &PipelineLayout {
        &self.tex_layout
    }

    pub fn alloc_descriptor_set(&mut self, shader: BuiltinShader) -> Result<DescriptorSet, Error> {
        match shader {
            BuiltinShader::Clear => self.clear_desc_pool.alloc_descriptor_set().map_err(Into::into),
            BuiltinShader::Texture => self.tex_desc_pool.alloc_descriptor_set().map_err(Into::into),
        }
    }
}

impl Drop for Pipelines {
    fn drop(&mut self) {
        if let Some(device) = self.device.upgrade() {
            unsafe {
                device.vk().destroy_pipeline_layout(self.clear_layout, None);
                device.vk().destroy_pipeline(self.clear_pipeline, None);
                device.vk().destroy_pipeline_layout(self.tex_layout, None);
                device.vk().destroy_pipeline(self.tex_pipeline, None);
                device.vk().destroy_pipeline_cache(self.cache, None);
            }
        }
    }
}
