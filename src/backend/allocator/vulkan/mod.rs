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

pub mod format;

use std::{
    ffi::CStr,
    fmt,
    os::unix::io::{FromRawFd, OwnedFd},
    sync::{mpsc, Arc, Weak},
};

use ash::{
    extensions::{ext, khr},
    vk,
};
use bitflags::bitflags;
use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};
use tracing::instrument;

#[cfg(feature = "backend_drm")]
use crate::backend::drm::DrmNode;
use crate::{
    backend::{
        allocator::dmabuf::DmabufFlags,
        vulkan::{version::Version, PhysicalDevice},
    },
    utils::{Buffer as BufferCoord, Size},
};

use super::{
    dmabuf::{AsDmabuf, Dmabuf, MAX_PLANES},
    Allocator, Buffer,
};

bitflags! {
    /// Flags to indicate the intended usage for a buffer.
    ///
    /// The usage flags may influence whether a buffer with a specific format and modifier combination may
    /// successfully be imported by a graphics api when it is exported as a [`Dmabuf`].
    ///
    /// For example, if a [`Buffer`] is used for scan-out, it is only necessary to specify the color
    /// attachment usage. However, if the exported buffer is used by a client and intended to be rendered to,
    /// then it is possible that the import will fail because the buffer was not allocated with right usage
    /// for the format and modifier combination.
    ///
    /// The default usage set when creating a [`VulkanAllocator`] guarantees that an exported buffer may be
    /// imported successfully with the same usage.
    ///
    /// If you need to allocate buffers with different usages dynamically, then you may use
    /// [`VulkanAllocator::create_buffer_with_usage`].
    ///
    /// [`VulkanAllocator::is_format_supported`] can check if the combination of format, modifier and usage
    /// flags are supported.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct ImageUsageFlags: vk::Flags {
        /// The image may be the source of a transfer command.
        ///
        /// This allows the content of the exported buffer to be downloaded from the GPU.
        const TRANSFER_SRC = vk::ImageUsageFlags::TRANSFER_SRC.as_raw();

        /// The image may be the destination of a transfer command.
        ///
        /// This allows the content of the exported buffer to be modified by a memory upload to the GPU.
        const TRANSFER_DST = vk::ImageUsageFlags::TRANSFER_DST.as_raw();

        /// Image may be sampled in a shader.
        ///
        /// This should be used if the exported buffer will be used as a texture.
        const SAMPLED = vk::ImageUsageFlags::SAMPLED.as_raw();

        /// The image may be used in a color attachment.
        ///
        /// This should be used if the exported buffer will be rendered to.
        const COLOR_ATTACHMENT = vk::ImageUsageFlags::COLOR_ATTACHMENT.as_raw();
    }
}

/// Error type for [`VulkanAllocator`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The allocator could not be created.
    ///
    /// This can occur for a few reasons:
    /// - No suitable queue family was found.
    #[error("could not create allocator")]
    Setup,

    /// The size specified to create the buffer was too small.
    ///
    /// The size must be greater than `0 x 0` pixels.
    #[error("invalid buffer size")]
    InvalidSize,

    /// The specified format is not supported.
    ///
    /// This error may occur for a few reasons:
    /// 1. There is no equivalent Vulkan format for the specified format code.
    /// 2. The driver does not support any of the specified modifiers for the format code.
    /// 3. The driver does support some of the format, but the size of the buffer requested is too large.
    /// 4. The usage is empty
    #[error("format is not supported")]
    UnsupportedFormat,

    /// Some error from the Vulkan driver.
    #[error(transparent)]
    Vk(#[from] vk::Result),
}

/// An allocator which uses Vulkan to create buffers.
pub struct VulkanAllocator {
    formats: Vec<FormatEntry>,
    images: Vec<ImageInner>,
    default_usage: ImageUsageFlags,
    remaining_allocations: u32,
    extension_fns: ExtensionFns,
    dropped_recv: mpsc::Receiver<ImageInner>,
    dropped_sender: mpsc::Sender<ImageInner>,
    phd: PhysicalDevice,
    #[cfg(feature = "backend_drm")]
    node: Option<DrmNode>,
    device: Arc<ash::Device>,
}

impl fmt::Debug for VulkanAllocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VulkanAllocator")
            .field("formats", &self.formats)
            .field("images", &self.images)
            .field("default_usage", &self.default_usage)
            .field("remaining_allocations", &self.remaining_allocations)
            .field("dropped_recv", &self.dropped_recv)
            .field("dropped_sender", &self.dropped_sender)
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
            vk::ExtImageDrmFormatModifierFn::name(),
            vk::ExtExternalMemoryDmaBufFn::name(),
            vk::KhrExternalMemoryFdFn::name(),
        ];

        if phd.api_version() < Version::VERSION_1_2 {
            // VK_EXT_image_drm_format_modifier requires VK_KHR_image_format_list.
            // VK_KHR_image_format_list is part of the core API in Vulkan 1.2
            extensions.push(vk::KhrImageFormatListFn::name());
        }

        // Optional extensions:

        // VK_EXT_4444_formats is part of the core API in Vulkan 1.3. Although not always supported
        // (see the 1.3 features to enable)
        if phd.api_version() < Version::VERSION_1_3 {
            // In 1.2 and below, the device must support the extension to use it.
            if phd.has_device_extension(vk::Ext4444FormatsFn::name()) {
                extensions.push(vk::Ext4444FormatsFn::name());
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

        // Get required extensions
        let extensions = Self::required_extensions(phd);
        let extension_pointers = extensions.iter().copied().map(CStr::as_ptr).collect::<Vec<_>>();

        // We don't actually submit any commands to the queue, but Vulkan requires that we create devices with
        // at least one queue (VUID-VkDeviceCreateInfo-queueCreateInfoCount-arraylength).
        let queue_families = unsafe {
            phd.instance()
                .handle()
                .get_physical_device_queue_family_properties(phd.handle())
        };

        let queue_family_index = queue_families
            .iter()
            // Find a queue with transfer
            .position(|properties| properties.queue_flags.contains(vk::QueueFlags::TRANSFER))
            // If there is no transfer queue family, then try graphics (which must support transfer)
            .or_else(|| {
                queue_families
                    .iter()
                    .position(|properties| properties.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            })
            .ok_or(Error::Setup)?;

        // TODO: Enable 4444 formats features
        let queue_create_info = [vk::DeviceQueueCreateInfo::builder()
            .queue_family_index(queue_family_index as u32)
            .queue_priorities(&[0.0])
            .build()];
        let create_info = vk::DeviceCreateInfo::builder()
            .enabled_extension_names(&extension_pointers)
            .queue_create_infos(&queue_create_info);

        let instance = phd.instance().handle();
        let device = unsafe { instance.create_device(phd.handle(), &create_info, None) }?;

        // Load extension functions
        let extension_fns = ExtensionFns {
            ext_image_format_modifier: ext::ImageDrmFormatModifier::new(instance, &device),
            khr_external_memory_fd: khr::ExternalMemoryFd::new(instance, &device),
        };

        let (dropped_sender, dropped_recv) = mpsc::channel();

        #[cfg(feature = "backend_drm")]
        let node = phd
            .render_node()
            .ok()
            .flatten()
            .or_else(|| phd.primary_node().ok().flatten());

        let mut allocator = VulkanAllocator {
            formats: Vec::new(),
            images: Vec::new(),
            default_usage,
            remaining_allocations: phd.limits().max_memory_allocation_count,
            extension_fns,
            dropped_recv,
            dropped_sender,
            phd: phd.clone(),
            #[cfg(feature = "backend_drm")]
            node,
            device: Arc::new(device),
        };

        allocator.init_formats();

        Ok(allocator)
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
        unsafe { self.get_format_info(format, vk::ImageUsageFlags::from_raw(usage.bits())) }
            .ok()
            .is_some()
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
        self.cleanup();

        let vk_format = format::get_vk_format(fourcc).ok_or(Error::UnsupportedFormat)?;
        let vk_usage = vk::ImageUsageFlags::from_raw(usage.bits());

        // VUID-VkImageCreateInfo-extent-00944, VUID-VkImageCreateInfo-extent-00945
        if width == 0 || height == 0 {
            return Err(Error::InvalidSize);
        }

        // VUID-VkPhysicalDeviceImageFormatInfo2-usage-requiredbitmask
        // At least one image usage flag must be specified.
        if usage.is_empty() {
            return Err(Error::UnsupportedFormat);
        }

        // VUID-VkImageCreateInfo-usage-00964, VUID-VkImageCreateInfo-usage-00965
        if usage.contains(ImageUsageFlags::COLOR_ATTACHMENT) {
            let limits = self.phd.limits();

            if width > limits.max_framebuffer_width || height > limits.max_framebuffer_height {
                return Err(Error::InvalidSize);
            }
        }

        // Filter out any format + modifier combinations that are not supported
        let modifiers = self.filter_modifiers(width, height, vk_usage, fourcc, modifiers);

        // VUID-VkImageDrmFormatModifierListCreateInfoEXT-drmFormatModifierCount-arraylength
        if modifiers.is_empty() {
            return Err(Error::UnsupportedFormat);
        }

        Ok(unsafe { self.create_image(width, height, vk_format, vk_usage, fourcc, &modifiers[..]) }?)
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

impl Drop for VulkanAllocator {
    fn drop(&mut self) {
        unsafe {
            for image in &self.images {
                self.device.destroy_image(image.image, None);
                self.device.free_memory(image.memory, None);
            }

            self.device.destroy_device(None);
        }
    }
}

/// Vulkan image object.
///
/// This type implements [`Buffer`] and the underlying image may be exported as a dmabuf.
pub struct VulkanImage {
    inner: ImageInner,
    width: u32,
    height: u32,
    format: DrmFormat,
    #[cfg(feature = "backend_drm")]
    node: Option<DrmNode>,
    /// The number of planes the image has for dmabuf export.
    format_plane_count: u32,
    khr_external_memory_fd: khr::ExternalMemoryFd,
    dropped_sender: mpsc::Sender<ImageInner>,
    device: Weak<ash::Device>,
}

impl fmt::Debug for VulkanImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VulkanImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("inner", &self.inner)
            .finish()
    }
}

impl Buffer for VulkanImage {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn size(&self) -> Size<i32, BufferCoord> {
        (self.width as i32, self.height as i32).into()
    }

    fn format(&self) -> DrmFormat {
        self.format
    }
}

impl AsDmabuf for VulkanImage {
    type Error = ExportError;

    #[profiling::function]
    fn export(&self) -> Result<Dmabuf, Self::Error> {
        let device = self.device.upgrade().ok_or(ExportError::AllocatorDestroyed)?;

        // Implementation may be broken if the plane count is wrong.
        if self.format_plane_count == 0 {
            return Err(ExportError::Failed);
        }

        assert!(
            self.format_plane_count as usize <= MAX_PLANES,
            "Vulkan implementation reported too many planes"
        );

        let create_info = vk::MemoryGetFdInfoKHR::builder()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            // VUID-VkMemoryGetFdInfoKHR-handleType-00671: Memory was allocated with DMA_BUF_EXT
            .memory(self.inner.memory);

        let fd = unsafe { self.khr_external_memory_fd.get_memory_fd(&create_info) }?;
        let mut builder = Dmabuf::builder(
            self.size(),
            self.format().code,
            self.format().modifier,
            DmabufFlags::empty(),
        );

        for idx in 0..self.format_plane_count {
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
            let subresource = vk::ImageSubresource::builder().aspect_mask(aspect_mask).build();
            let layout = unsafe { device.get_image_subresource_layout(self.inner.image, subresource) };
            builder.add_plane(
                // SAFETY: `vkGetMemoryFdKHR` creates a new file descriptor owned by the caller.
                unsafe { OwnedFd::from_raw_fd(fd) },
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

impl Drop for VulkanImage {
    fn drop(&mut self) {
        let _ = self.dropped_sender.send(self.inner);
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

#[derive(Debug, Clone, Copy, PartialEq)]
struct ImageInner {
    // TODO: image usage?
    image: vk::Image,
    /// The first entry will always have a non-null device memory handle.
    ///
    /// The other three entries may be a null handle or valid device memory (the latter with disjoint dmabufs).
    memory: vk::DeviceMemory,
}

#[derive(Debug)]
struct FormatEntry {
    format: DrmFormat,
    modifier_properties: vk::DrmFormatModifierPropertiesEXT,
}

struct ExtensionFns {
    /// Functions to get DRM format information.
    ///
    /// These are required, and therefore you may assume this functionality is available.
    ext_image_format_modifier: ext::ImageDrmFormatModifier,
    /// Functions used for dmabuf import and export.
    ///
    /// If this is [`Some`], then the allocator will support dmabuf import and export operations.
    khr_external_memory_fd: khr::ExternalMemoryFd,
}

impl VulkanAllocator {
    fn init_formats(&mut self) {
        for &fourcc in format::known_formats() {
            let vk_format = format::get_vk_format(fourcc).unwrap();
            let modifier_properties = self
                .phd
                .get_format_modifier_properties(vk_format)
                .expect("The Vulkan allocator requires VK_EXT_image_drm_format_modifier");

            for modifier_properties in modifier_properties {
                self.formats.push(FormatEntry {
                    format: DrmFormat {
                        code: fourcc,
                        modifier: DrmModifier::from(modifier_properties.drm_format_modifier),
                    },
                    modifier_properties,
                });
            }
        }
    }

    /// Returns whether the format + modifier combination and the usage flags are supported.
    unsafe fn get_format_info(
        &self,
        format: DrmFormat,
        usage: vk::ImageUsageFlags,
    ) -> Result<Option<vk::ImageFormatProperties>, vk::Result> {
        let vk_format = format::get_vk_format(format.code);

        match vk_format {
            Some(vk_format) => {
                let mut image_drm_format_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::builder()
                    .drm_format_modifier(format.modifier.into())
                    .sharing_mode(vk::SharingMode::EXCLUSIVE);
                let format_info = vk::PhysicalDeviceImageFormatInfo2::builder()
                    .format(vk_format)
                    .ty(vk::ImageType::TYPE_2D)
                    .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                    .usage(usage)
                    .flags(vk::ImageCreateFlags::empty())
                    // VUID-VkPhysicalDeviceImageFormatInfo2-tiling-02249
                    .push_next(&mut image_drm_format_info);
                let mut image_format_properties = vk::ImageFormatProperties2::default();

                // VUID-vkGetPhysicalDeviceImageFormatProperties-tiling-02248: Must use vkGetPhysicalDeviceImageFormatProperties2
                let result = unsafe {
                    self.phd
                        .instance()
                        .handle()
                        .get_physical_device_image_format_properties2(
                            self.phd.handle(),
                            &format_info,
                            &mut image_format_properties,
                        )
                };

                result
                    .map(|_| Some(image_format_properties.image_format_properties))
                    .or_else(|result| {
                        // Unsupported format + usage combination
                        if result == vk::Result::ERROR_FORMAT_NOT_SUPPORTED {
                            Ok(None)
                        } else {
                            Err(result)
                        }
                    })
            }

            None => Ok(None),
        }
    }

    fn filter_modifiers(
        &self,
        width: u32,
        height: u32,
        vk_usage: vk::ImageUsageFlags,
        fourcc: DrmFourcc,
        modifiers: &[DrmModifier],
    ) -> Vec<u64> {
        modifiers
            .iter()
            .copied()
            .filter_map(move |modifier| {
                let info = unsafe {
                    self.get_format_info(
                        DrmFormat {
                            code: fourcc,
                            modifier,
                        },
                        vk_usage,
                    )
                }
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
            .map(Into::<u64>::into)
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
        vk_format: vk::Format,
        vk_usage: vk::ImageUsageFlags,
        fourcc: DrmFourcc,
        modifiers: &[u64],
    ) -> Result<VulkanImage, vk::Result> {
        assert!(width > 0);
        assert!(height > 0);

        // Ensure maximum allocations are not exceeded.
        if self.remaining_allocations == 0 {
            todo!()
        }

        // Now that the list of valid modifiers is known, create an image using one of the modifiers.
        let mut modifier_list =
            vk::ImageDrmFormatModifierListCreateInfoEXT::builder().drm_format_modifiers(modifiers);
        let mut external_memory_image_create_info = vk::ExternalMemoryImageCreateInfo::builder()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

        let image_create_info = vk::ImageCreateInfo::builder()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D {
                width,
                height,
                // VUID-VkImageCreateInfo-extent-00946
                // VUID-VkImageCreateInfo-imageType-00957
                depth: 1,
            })
            // VUID-VkImageCreateInfo-samples-parameter
            .samples(vk::SampleCountFlags::TYPE_1)
            // VUID-VkImageCreateInfo-mipLevels-00947
            .mip_levels(1)
            // VUID-VkImageCreateInfo-arrayLayers-00948
            .array_layers(1)
            // VUID-VkImageCreateInfo-pNext-02262
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            // VUID-VkImageCreateInfo-usage-requiredbitmask
            // FIXME: We don't assert any usage flags are set
            .usage(vk_usage)
            // VUID-VkImageCreateInfo-initialLayout-00993
            .initial_layout(vk::ImageLayout::UNDEFINED)
            // VUID-VkImageCreateInfo-tiling-02261
            .push_next(&mut modifier_list)
            // TODO: VUID-VkImageCreateInfo-pNext-00990
            .push_next(&mut external_memory_image_create_info);

        // The image is placed in a scope guard to safely handle future allocation failures.
        let mut guard = scopeguard::guard(
            ImageInner {
                // This is the only spot where ? may be used to detect and error since no previous handles have been created.
                image: unsafe { self.device.create_image(&image_create_info, None) }?,
                memory: vk::DeviceMemory::null(),
            },
            |inner| unsafe { self.device.destroy_image(inner.image, None) },
        );

        // Get the modifier Vulkan created the image using.
        let format = {
            let mut image_modifier_properties = vk::ImageDrmFormatModifierPropertiesEXT::default();

            unsafe {
                self.extension_fns
                    .ext_image_format_modifier
                    .get_image_drm_format_modifier_properties(guard.image, &mut image_modifier_properties)
            }?;

            DrmFormat {
                code: fourcc,
                modifier: DrmModifier::from(image_modifier_properties.drm_format_modifier),
            }
        };

        // Now that we know the plane count, get the number of planes for the format + modifier
        let format_plane_count = self
            .formats
            .iter()
            .find(|entry| entry.format == format)
            .unwrap()
            .modifier_properties
            .drm_format_modifier_plane_count;

        // Allocate image memory
        let memory_reqs = unsafe { self.device.get_image_memory_requirements(guard.image) };
        // TODO: Memory type index
        let mut export_memory_allocate_info = vk::ExportMemoryAllocateInfo::builder()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let alloc_create_info = vk::MemoryAllocateInfo::builder()
            .allocation_size(memory_reqs.size)
            .push_next(&mut export_memory_allocate_info);

        unsafe {
            // Allocate memory for the image.
            guard.memory = self.device.allocate_memory(&alloc_create_info, None)?;
            // Finally bind the memory to the image
            self.device.bind_image_memory(guard.image, guard.memory, 0)?;
        }

        // Initialization is complete, prevent the scope guard from running it's dropfn.
        let inner = scopeguard::ScopeGuard::into_inner(guard);

        // Track the image for destruction.
        self.images.push(inner);

        self.remaining_allocations -= 1;

        Ok(VulkanImage {
            inner,
            width,
            height,
            format,
            format_plane_count,
            khr_external_memory_fd: self.extension_fns.khr_external_memory_fd.clone(),
            dropped_sender: self.dropped_sender.clone(),
            device: Arc::downgrade(&self.device),
            #[cfg(feature = "backend_drm")]
            node: self.node,
        })
    }

    fn cleanup(&mut self) {
        let dropped = self.dropped_recv.try_iter().collect::<Vec<_>>();

        self.images.retain(|image| {
            // Only drop if the
            let drop = dropped.contains(image);

            if drop {
                // Destroy the underlying image resource
                unsafe {
                    self.device.destroy_image(image.image, None);
                    self.device.free_memory(image.memory, None);
                }

                self.remaining_allocations = self
                    .remaining_allocations
                    .checked_add(1)
                    .expect("Remaining allocations overflowed");
                debug_assert!(
                    self.phd.limits().max_memory_allocation_count >= self.remaining_allocations,
                    "Too many allocations released",
                );
            }

            // If the image was dropped, return false
            !drop
        })
    }
}
