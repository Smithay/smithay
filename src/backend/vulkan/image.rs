use std::{fmt, io, os::fd::IntoRawFd, sync::Arc};

pub use ash::vk::ImageUsageFlags;
use ash::vk::{self, ImageTiling, MemoryPropertyFlags};
#[cfg(feature = "backend_drm")]
use drm::node::DrmNode;

use super::device::WeakDevice;
use crate::backend::{
    allocator::{dmabuf::Dmabuf, format::has_alpha, Buffer, Format, Fourcc, Modifier},
    vulkan::{format::component_mapping_for_format, Device},
};

/// Vulkan image object.
///
/// The underlying image may be exportable as a dmabuf.
#[derive(Clone)]
pub struct VulkanImage {
    pub(crate) inner: Arc<ImageInner>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: vk::Format,
    pub(crate) has_alpha: bool,
    pub(crate) mem_types: MemoryPropertyFlags,
    pub(crate) usage: ImageUsageFlags,
    pub(crate) tiling: ImageTiling,
    pub(crate) drm: Option<Format>,
    #[cfg(feature = "backend_drm")]
    pub(crate) node: Option<DrmNode>,
}

impl fmt::Debug for VulkanImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VulkanImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("has_alpha", &self.has_alpha)
            .field("usage", &self.usage)
            .field("drm", &self.drm)
            .field("inner", &self.inner)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
/// Errors created when creating or manipulating `VulkanImage`s.
pub enum Error {
    #[error("Size provided isn't valid")]
    InvalidSize,
    #[error("Format not supported")]
    UnsupportedFormat,
    #[error("Invalid or unsupported (distinct planes) dmabuf")]
    UnsupportedDmabuf,
    #[error("Missing or invalid image usage flags for the requested operation")]
    MissingOrInvalidUsage,
    #[error("Unable to find a supported memory type for the allocation")]
    NoMemoryAvailable,
    #[error("Creating vulkan image failed")]
    VulkanImage(#[source] vk::Result),
    #[error("Querying the vulkan image's modifier failed")]
    VulkanModifierQuery(#[source] vk::Result),
    #[error("Allocating vulkan device memory for the image failed")]
    VulkanAllocate(#[source] vk::Result),
    #[error("Binding the vulkan device memory to the image failed")]
    VulkanBind(#[source] vk::Result),
    #[error("Creating a vulkan image view")]
    VulkanImageView(#[source] vk::Result),
    #[error("Failed to clone the dmabuf fd")]
    DmabufFdError(#[source] io::Error),
}

impl VulkanImage {
    pub fn new(
        device: &Device,
        width: u32,
        height: u32,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
        linear: bool,
    ) -> Result<Self, Error> {
        Self::new_internal(
            device,
            width,
            height,
            format,
            usage,
            linear,
            Option::<(_, Option<Modifier>)>::None,
            None,
        )
    }

    pub fn new_exportable(
        device: &Device,
        width: u32,
        height: u32,
        format: Fourcc,
        modifiers: impl Iterator<Item = Modifier>,
        usage: vk::ImageUsageFlags,
    ) -> Result<Self, Error> {
        let vk_format = super::format::get_vk_format(format).ok_or(Error::UnsupportedFormat)?;
        Self::new_internal(
            device,
            width,
            height,
            vk_format,
            usage,
            false,
            Some((format, modifiers)),
            None,
        )
    }

    pub fn new_from_dmabuf(
        device: &Device,
        dmabuf: &Dmabuf,
        usage: vk::ImageUsageFlags,
    ) -> Result<Self, Error> {
        let width = dmabuf.width();
        let height = dmabuf.height();
        let vk_format = super::format::get_vk_format(dmabuf.format().code).ok_or(Error::UnsupportedFormat)?;

        // TODO: Handle distinct dmabuf formats
        // (see vkBindImageMemory2 and VkBindImagePlaneMemoryInfo)
        let handles = dmabuf.handles().collect::<Vec<_>>();
        let ino = rustix::fs::fstat(handles[0])
            .map_err(|_| Error::UnsupportedDmabuf)?
            .st_ino;
        if handles
            .iter()
            .skip(1)
            .any(|h| rustix::fs::fstat(h).is_ok_and(|s| s.st_ino != ino))
        {
            return Err(Error::UnsupportedFormat);
        }

        Self::new_internal(
            device,
            width,
            height,
            vk_format,
            usage,
            false,
            Option::<(_, Option<Modifier>)>::None,
            Some(dmabuf),
        )
    }

    fn new_internal(
        device: &Device,
        width: u32,
        height: u32,
        vk_format: vk::Format,
        vk_usage: vk::ImageUsageFlags,
        linear: bool,
        modifiers: Option<(Fourcc, impl IntoIterator<Item = Modifier>)>,
        dmabuf: Option<&Dmabuf>,
    ) -> Result<Self, Error> {
        let (fourcc, modifiers) = modifiers
            .map(|(fourcc, modifiers)| (fourcc, modifiers.into_iter().map(u64::from).collect::<Vec<_>>()))
            .unzip();
        let has_alpha = fourcc.is_none_or(|fourcc| has_alpha(fourcc));
        let mut modifier_list = modifiers.as_deref().map(|modifiers| {
            vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(modifiers)
        });
        let tiling = modifiers
            .as_deref()
            .map(|_| vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .unwrap_or_else(|| {
                if linear {
                    vk::ImageTiling::LINEAR
                } else {
                    vk::ImageTiling::OPTIMAL
                }
            });

        let mut image_create_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .samples(vk::SampleCountFlags::TYPE_1)
            .mip_levels(1)
            .array_layers(1)
            .usage(vk_usage)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .tiling(tiling);

        let plane_layouts: Vec<vk::SubresourceLayout>;
        let mut modifier_image_create_info: vk::ImageDrmFormatModifierExplicitCreateInfoEXT<'_>;
        let mut external_image_create_info: vk::ExternalMemoryImageCreateInfo<'_>;

        if let Some(modifier_list) = modifier_list
            .as_mut()
            .filter(|_| tiling == vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        {
            image_create_info = image_create_info.push_next(modifier_list);
        }

        if let Some(dmabuf) = dmabuf {
            plane_layouts = dmabuf
                .offsets()
                .zip(dmabuf.strides())
                .map(|(offset, stride)| {
                    vk::SubresourceLayout::default()
                        .offset(offset as u64)
                        .row_pitch(stride as u64)
                })
                .collect::<Vec<_>>();

            modifier_image_create_info = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
                .drm_format_modifier(dmabuf.format().modifier.into())
                .plane_layouts(&plane_layouts);

            image_create_info = image_create_info.push_next(&mut modifier_image_create_info);
        };

        if modifiers.is_some() || dmabuf.is_some() {
            external_image_create_info = vk::ExternalMemoryImageCreateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

            image_create_info = image_create_info.push_next(&mut external_image_create_info);
        }

        let mut inner = ImageInner {
            image: unsafe {
                device
                    .vk()
                    .create_image(&image_create_info, None)
                    .map_err(Error::VulkanImage)?
            },
            memory: vk::DeviceMemory::null(),
            device: device.downgrade(),
            view: None,
            dmabuf_exportable: modifiers.is_some() || dmabuf.is_some(),
            dmabuf_plane_count: dmabuf.map(|dmabuf| dmabuf.num_planes() as u32).unwrap_or(0),
        };

        let drm_format = fourcc
            .map(|fourcc| {
                let format = {
                    let mut image_modifier_properties = vk::ImageDrmFormatModifierPropertiesEXT::default();

                    unsafe {
                        device
                            .vk_ext_image_drm_format_modifier()
                            .expect("Required extensions contains ext_image_format_modifier")
                            .get_image_drm_format_modifier_properties(
                                inner.image,
                                &mut image_modifier_properties,
                            )
                            .map_err(Error::VulkanModifierQuery)?
                    };

                    Format {
                        code: fourcc,
                        modifier: Modifier::from(image_modifier_properties.drm_format_modifier),
                    }
                };

                // Now that we know the format, get the number of planes
                let format_plane_count = device
                    .formats()
                    .find(|entry| entry.format == format)
                    .unwrap()
                    .modifier_properties
                    .drm_format_modifier_plane_count;
                inner.dmabuf_plane_count = format_plane_count;

                Ok(format)
            })
            .transpose()?;

        // Allocate image memory
        let memory_reqs = unsafe { device.vk().get_image_memory_requirements(inner.image) };
        // TODO: Memory type index
        let mut alloc_create_info = vk::MemoryAllocateInfo::default().allocation_size(memory_reqs.size);

        let mut mem_bits = None;
        for (i, types) in device
            .memory_properties()
            .memory_types_as_slice()
            .iter()
            .enumerate()
        {
            if memory_reqs.memory_type_bits & (i as u32) != 0
                && types.property_flags.contains(MemoryPropertyFlags::DEVICE_LOCAL)
            {
                alloc_create_info = alloc_create_info.memory_type_index(i as u32);
                mem_bits = Some(types.property_flags.clone());
                break;
            }
        }

        let Some(mem_bits) = mem_bits else {
            return Err(Error::NoMemoryAvailable);
        };

        let mut import_memory_info: vk::ImportMemoryFdInfoKHR<'_>;
        let mut memory_export_info: vk::ExportMemoryAllocateInfo<'_>;
        let mut memory_dedicated_info: vk::MemoryDedicatedAllocateInfo<'_>;

        if modifiers.is_some() || dmabuf.is_some() {
            memory_dedicated_info = vk::MemoryDedicatedAllocateInfo::default().image(inner.image);
            alloc_create_info = alloc_create_info.push_next(&mut memory_dedicated_info);
        }

        if inner.dmabuf_exportable {
            memory_export_info = vk::ExportMemoryAllocateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
            alloc_create_info = alloc_create_info.push_next(&mut memory_export_info);
        }

        if let Some(dmabuf) = dmabuf {
            // TODO: Distinct planes. See `new_from_dmabuf`
            let handle = dmabuf.handles().next().unwrap();

            import_memory_info = vk::ImportMemoryFdInfoKHR::default()
                .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
                .fd(handle
                    .try_clone_to_owned()
                    .map_err(Error::DmabufFdError)?
                    .into_raw_fd());
            alloc_create_info = alloc_create_info.push_next(&mut import_memory_info);
        }

        unsafe {
            // Allocate memory for the image.
            // TODO: In case of error close the fd in import_memory_info
            inner.memory = device
                .vk()
                .allocate_memory(&alloc_create_info, None)
                .map_err(Error::VulkanAllocate)?;
            // Finally bind the memory to the image
            device
                .vk()
                .bind_image_memory(inner.image, inner.memory, 0)
                .map_err(Error::VulkanBind)?;
        }

        if vk_usage.contains(vk::ImageUsageFlags::SAMPLED) || vk_usage.contains(vk::ImageUsageFlags::STORAGE)
        {
            let info = vk::ImageViewCreateInfo::default()
                .image(inner.image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk_format)
                .components(if vk_usage.contains(vk::ImageUsageFlags::STORAGE) {
                    vk::ComponentMapping {
                        r: vk::ComponentSwizzle::IDENTITY,
                        g: vk::ComponentSwizzle::IDENTITY,
                        b: vk::ComponentSwizzle::IDENTITY,
                        a: vk::ComponentSwizzle::IDENTITY,
                    }
                } else {
                    component_mapping_for_format(vk_format, has_alpha)
                })
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                );
            inner.view = Some(unsafe {
                device
                    .vk()
                    .create_image_view(&info, None)
                    .map_err(Error::VulkanImageView)?
            });
        }

        Ok(VulkanImage {
            inner: Arc::new(inner),
            width,
            height,
            format: vk_format,
            drm: drm_format.or_else(|| {
                super::format::get_drm_format(vk_format).map(|fourcc| Format {
                    code: fourcc,
                    modifier: Modifier::Invalid,
                })
            }),
            mem_types: mem_bits,
            has_alpha,
            usage: vk_usage,
            tiling,
            #[cfg(feature = "backend_drm")]
            node: if let Some(dmabuf) = dmabuf {
                dmabuf.node()
            } else {
                device.node()
            },
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn format(&self) -> vk::Format {
        self.format
    }

    pub fn vk(&self) -> &vk::Image {
        &self.inner.image
    }

    pub fn vk_view(&self) -> Option<&vk::ImageView> {
        self.inner.view.as_ref()
    }

    pub fn vk_usage(&self) -> ImageUsageFlags {
        self.usage
    }

    pub fn mem_bits(&self) -> MemoryPropertyFlags {
        self.mem_types
    }

    pub fn is_linear(&self) -> bool {
        self.tiling == ImageTiling::LINEAR
            || (self.tiling == ImageTiling::DRM_FORMAT_MODIFIER_EXT
                && self
                    .drm
                    .as_ref()
                    .is_some_and(|format| format.modifier == Modifier::Linear))
    }
}

#[derive(Debug)]
pub(crate) struct ImageInner {
    pub(crate) image: vk::Image,
    // might be a null handle
    pub(crate) memory: vk::DeviceMemory,
    pub(crate) device: WeakDevice,
    pub(crate) view: Option<vk::ImageView>,

    pub(crate) dmabuf_exportable: bool,
    pub(crate) dmabuf_plane_count: u32,
}

impl Drop for ImageInner {
    fn drop(&mut self) {
        if let Some(device) = self.device.upgrade() {
            unsafe {
                let vk = device.vk();
                if let Some(view) = self.view.as_ref() {
                    vk.destroy_image_view(*view, None);
                }
                vk.destroy_image(self.image, None);
                vk.free_memory(self.memory, None);
            }
        }
    }
}
