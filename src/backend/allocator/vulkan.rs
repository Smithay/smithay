//! Module for Buffers created using Vulkan.
//!
//! The [`VulkanAllocator`] type implements the [`Allocator`] trait and [`VulkanImage`] implements [`Buffer`].
//! A [`VulkanImage`] may be exported as a [dmabuf](super::dmabuf).
//!
//! The Vulkan allocator supports up to Vulkan 1.3.
//!
//! The Vulkan allocator requires the following device extensions (and their dependencies):
//! - `VK_EXT_image_drm_format_modifier`
//! - `VK_EXT_external_memory_dmabuf`
//! - `VK_KHR_external_memory_fd`
//!
//! Additionally the Vulkan allocator may enable the following extensions if available:
//! - `VK_EXT_4444_formats`
//!
//! To get the required extensions a device must support to use the Vulkan allocator, use
//! [`VulkanAllocator::required_extensions`].

#![forbid(unsafe_op_in_unsafe_fn)]

use std::{
    ffi::CStr,
    fmt,
    os::unix::io::{FromRawFd, OwnedFd},
};

use ash::{
    ext, khr,
    vk::{self, PhysicalDeviceFeatures2},
};
use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};
use tracing::instrument;

use crate::{
    backend::{
        allocator::{dmabuf::DmabufFlags, format::has_alpha},
        renderer::vulkan::VulkanRenderer,
        vulkan::{
            device::{Device, DeviceError, QueueType},
            image::{Error as ImageError, ImageUsageFlags, VulkanImage},
            version::Version,
            PhysicalDevice,
        },
    },
    utils::{Buffer as BufferCoord, Size},
};

use super::{
    dmabuf::{AsDmabuf, Dmabuf, MAX_PLANES},
    Allocator, Buffer,
};

/// Error type for [`VulkanAllocator`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The vulkan device could not be created.
    #[error(transparent)]
    Device(#[from] DeviceError),

    /// The vulkan image could not be created.
    #[error(transparent)]
    Image(#[from] ImageError),

    /// Some error from the Vulkan driver.
    #[error(transparent)]
    Vk(#[from] vk::Result),
}

/// An allocator which uses Vulkan to create buffers.
pub struct VulkanAllocator {
    default_usage: ImageUsageFlags,
    phd: PhysicalDevice,
    device: Device,
}

impl fmt::Debug for VulkanAllocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VulkanAllocator")
            .field("default_usage", &self.default_usage)
            .field("phd", &self.phd)
            .finish()
    }
}

impl VulkanAllocator {
    /// Maximum supported version instance version that may be used with the allocator.
    pub const MAX_INSTANCE_VERSION: Version = Version::VERSION_1_3;

    /// Returns the list of device extensions required by the Vulkan allocator.
    ///
    /// This function may return a different list for each [`PhysicalDevice`], meaning each device should be
    /// filtered using it's own call to this function.
    pub fn required_extensions(phd: &PhysicalDevice) -> Vec<&'static CStr> {
        // Always required extensions
        let mut extensions = vec![
            ext::image_drm_format_modifier::NAME,
            ext::external_memory_dma_buf::NAME,
            khr::external_memory_fd::NAME,
        ];

        if phd.api_version() < Version::VERSION_1_2 {
            // VK_EXT_image_drm_format_modifier requires VK_KHR_image_format_list.
            // VK_KHR_image_format_list is part of the core API in Vulkan 1.2
            extensions.push(khr::image_format_list::NAME);
        }

        // Optional extensions:

        // VK_EXT_4444_formats is part of the core API in Vulkan 1.3. Although not always supported
        // (see the 1.3 features to enable)
        if phd.api_version() < Version::VERSION_1_3 {
            // In 1.2 and below, the device must support the extension to use it.
            if phd.has_device_extension(ext::_4444_formats::NAME) {
                extensions.push(ext::_4444_formats::NAME);
            }
        }

        extensions
    }

    /// Creates a [`VulkanAllocator`].
    ///
    /// # Panics
    ///
    /// - If the version of instance which created the [`PhysicalDevice`] is higher than [`VulkanAllocator::MAX_INSTANCE_VERSION`].
    /// - If the default [`ImageUsageFlags`] are empty.
    #[instrument(err, skip(phd), fields(physical_device = phd.name()))]
    pub fn new(phd: &PhysicalDevice, default_usage: ImageUsageFlags) -> Result<VulkanAllocator, Error> {
        // Panic if the instance version is too high
        if phd.instance().api_version() > Self::MAX_INSTANCE_VERSION {
            panic!("Exceeded maximum instance api version for VulkanAllocator (1.3 max)")
        }

        // VUID-VkPhysicalDeviceImageFormatInfo2-usage-requiredbitmask
        // At least one image usage flag must be specified.
        if default_usage.is_empty() {
            panic!("Default usage flags for allocator are empty")
        }

        let extensions = Self::required_extensions(phd);
        let mut features = PhysicalDeviceFeatures2::default();
        let device = Device::new(phd, &extensions, &mut features, QueueType::Transfer, true)?;

        let allocator = VulkanAllocator {
            default_usage,
            phd: phd.clone(),
            device,
        };

        Ok(allocator)
    }

    pub fn from_renderer(renderer: &VulkanRenderer, usage: ImageUsageFlags) -> Self {
        VulkanAllocator {
            default_usage: usage,
            phd: renderer.phd.clone(),
            device: renderer.device.clone(),
        }
    }

    /// Returns whether this allocator supports the specified format with the usage flags.
    pub fn is_format_supported(&self, format: DrmFormat, usage: ImageUsageFlags) -> bool {
        // VUID-VkPhysicalDeviceImageFormatInfo2-usage-requiredbitmask
        // At least one image usage flag must be specified.
        if usage.is_empty() {
            return false;
        }

        // TODO: Check if the extents are also valid?
        // Vulkan states a maximum extent size for images.
        // This may also be useful as a function on Allocator.
        self.phd.drm_format_info(format, usage).ok().is_some()
    }

    /// Try to create a buffer with the given dimensions, pixel format and usage flags.
    ///
    /// This may return [`Err`] for one of the following reasons:
    /// - The `usage` is empty.
    /// - The `fourcc` format is not supported.
    /// - All of the allowed `modifiers` are not supported.
    /// - The size of the buffer is too large for the `usage`, `fourcc` format or `modifiers`.
    /// - The `fourcc` format and `modifiers` do not support the specified usage.
    #[instrument(level = "trace", err)]
    #[profiling::function]
    pub fn create_buffer_with_usage(
        &mut self,
        width: u32,
        height: u32,
        fourcc: DrmFourcc,
        modifiers: &[DrmModifier],
        usage: ImageUsageFlags,
    ) -> Result<VulkanImage, Error> {
        // VUID-VkImageCreateInfo-extent-00944, VUID-VkImageCreateInfo-extent-00945
        if width == 0 || height == 0 {
            return Err(ImageError::InvalidSize.into());
        }

        // VUID-VkPhysicalDeviceImageFormatInfo2-usage-requiredbitmask
        // At least one image usage flag must be specified.
        if usage.is_empty() {
            return Err(ImageError::UnsupportedFormat.into());
        }

        // VUID-VkImageCreateInfo-usage-00964, VUID-VkImageCreateInfo-usage-00965
        if usage.contains(ImageUsageFlags::COLOR_ATTACHMENT) {
            let limits = self.phd.limits();

            if width > limits.max_framebuffer_width || height > limits.max_framebuffer_height {
                return Err(ImageError::InvalidSize.into());
            }
        }

        // Filter out any format + modifier combinations that are not supported
        let modifiers = self.filter_modifiers(width, height, usage, fourcc, modifiers);

        // VUID-VkImageDrmFormatModifierListCreateInfoEXT-drmFormatModifierCount-arraylength
        if modifiers.is_empty() {
            return Err(ImageError::UnsupportedFormat.into());
        }

        unsafe {
            self.create_image(width, height, usage, fourcc, modifiers.into_iter())
                .map_err(Into::into)
        }
    }

    /// Returns the [`PhysicalDevice`] this allocator was created with.
    pub fn physical_device(&self) -> &PhysicalDevice {
        &self.phd
    }
}

impl Allocator for VulkanAllocator {
    type Buffer = VulkanImage;
    type Error = Error;

    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: DrmFourcc,
        modifiers: &[DrmModifier],
    ) -> Result<VulkanImage, Self::Error> {
        self.create_buffer_with_usage(width, height, fourcc, modifiers, self.default_usage)
    }
}

impl Buffer for VulkanImage {
    fn width(&self) -> u32 {
        VulkanImage::width(self)
    }

    fn height(&self) -> u32 {
        VulkanImage::height(self)
    }

    fn size(&self) -> Size<i32, BufferCoord> {
        (VulkanImage::width(self) as i32, VulkanImage::height(self) as i32).into()
    }

    fn format(&self) -> DrmFormat {
        self.drm.unwrap()
    }
}

impl AsDmabuf for VulkanImage {
    type Error = ExportError;

    #[profiling::function]
    fn export(&self) -> Result<Dmabuf, Self::Error> {
        let device = self
            .inner
            .device
            .upgrade()
            .ok_or(ExportError::AllocatorDestroyed)?;

        // Was the image created exportable?
        if !self.inner.dmabuf_exportable || self.drm.is_none() {
            return Err(ExportError::Failed);
        }

        // Implementation may be broken if the plane count is wrong.
        if self.inner.dmabuf_plane_count == 0 {
            return Err(ExportError::Failed);
        }

        let Some(khr_external_memory_fd) = device.vk_khr_external_memory_fd() else {
            return Err(ExportError::Failed);
        };

        assert!(
            self.inner.dmabuf_plane_count as usize <= MAX_PLANES,
            "Vulkan implementation reported too many planes"
        );

        let create_info = vk::MemoryGetFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            // VUID-VkMemoryGetFdInfoKHR-handleType-00671: Memory was allocated with DMA_BUF_EXT
            .memory(self.inner.memory);

        let fd = unsafe { khr_external_memory_fd.get_memory_fd(&create_info) }?;
        // SAFETY: `vkGetMemoryFdKHR` creates a new file descriptor owned by the caller.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut builder = Dmabuf::builder(
            self.size(),
            self.drm.unwrap().code,
            self.drm.unwrap().modifier,
            DmabufFlags::empty(),
        );

        for idx in 0..self.inner.dmabuf_plane_count {
            // get_image_subresource_layout only gets the layout of one memory plane. This mask specifies
            // which plane should the layout be obtained for.
            let aspect_mask = match idx {
                0 => vk::ImageAspectFlags::MEMORY_PLANE_0_EXT,
                1 => vk::ImageAspectFlags::MEMORY_PLANE_1_EXT,
                2 => vk::ImageAspectFlags::MEMORY_PLANE_2_EXT,
                3 => vk::ImageAspectFlags::MEMORY_PLANE_3_EXT,
                _ => unreachable!(),
            };

            // VUID-vkGetImageSubresourceLayout-image-02270: All allocate images are created with drm tiling
            let subresource = vk::ImageSubresource::default().aspect_mask(aspect_mask);
            let layout = unsafe {
                device
                    .vk()
                    .get_image_subresource_layout(self.inner.image, subresource)
            };
            builder.add_plane(
                fd.try_clone().or(Err(ExportError::Failed))?,
                idx,
                layout.offset as u32,
                layout.row_pitch as u32,
            );
        }

        #[cfg(feature = "backend_drm")]
        if let Some(node) = self.node {
            builder.set_node(node);
        }

        Ok(builder.build().unwrap())
    }
}

/// The error type for exporting a [`VulkanImage`] as a [`Dmabuf`].
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The image could not export a dmabuf since the allocator has been destroyed.
    #[error("allocator has been destroyed")]
    AllocatorDestroyed,

    /// The allocator could not export a dmabuf for an implementation dependent reason.
    #[error("could not export a dmabuf")]
    Failed,

    /// Vulkan API error.
    #[error(transparent)]
    Vk(#[from] vk::Result),
}

impl VulkanAllocator {
    // TODO: move to src/backend/vulkan/image.rs VulkanImage::new_internal
    fn filter_modifiers(
        &self,
        width: u32,
        height: u32,
        vk_usage: vk::ImageUsageFlags,
        fourcc: DrmFourcc,
        modifiers: &[DrmModifier],
    ) -> Vec<DrmModifier> {
        modifiers
            .iter()
            .copied()
            .filter_map(move |modifier| {
                let info = self
                    .phd
                    .drm_format_info(
                        DrmFormat {
                            code: fourcc,
                            modifier,
                        },
                        vk_usage,
                    )
                    .ok()
                    .flatten()?;

                Some((modifier, info))
            })
            // Filter modifiers where the required image creation limits are not met
            .filter(move |(_, properties)| {
                let max_extent = properties.max_extent;

                // VUID-VkImageCreateInfo-extent-02252
                max_extent.width >= width
                // VUID-VkImageCreateInfo-extent-02253
                && max_extent.height >= height
                // VUID-VkImageCreateInfo-extent-02254
                // VUID-VkImageCreateInfo-extent-00946
                // VUID-VkImageCreateInfo-imageType-00957
                && max_extent.depth >= 1
                // VUID-VkImageCreateInfo-samples-02258
                && properties.sample_counts.contains(vk::SampleCountFlags::TYPE_1)
            })
            .map(|(modifier, _)| modifier)
            // TODO: Could use a smallvec or tinyvec to reduce number allocations
            .collect::<Vec<_>>()
    }

    /// # Safety
    ///
    /// * The list of modifiers must be supported for the given format and image usage flags.
    /// * The extent of the image must be within the maximum extents Vulkan tells.
    unsafe fn create_image(
        &mut self,
        width: u32,
        height: u32,
        vk_usage: vk::ImageUsageFlags,
        format: DrmFourcc,
        modifiers: impl Iterator<Item = DrmModifier>,
    ) -> Result<VulkanImage, ImageError> {
        assert!(width > 0);
        assert!(height > 0);

        // TODO: We cannot do anything but Swizzle::identity for storage.
        // Needs to be handled in the shader.
        if vk_usage.contains(vk::ImageUsageFlags::STORAGE) && !has_alpha(format) {
            return Err(ImageError::UnsupportedFormat);
        }

        VulkanImage::new_exportable(&self.device, width, height, format, modifiers, vk_usage)
    }
}
