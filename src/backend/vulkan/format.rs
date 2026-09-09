//! Format conversions between Vulkan and DRM formats.

use super::PhysicalDevice;
use crate::backend::allocator::{Format, Modifier};

use ash::vk;

/// Macro to generate format conversions between Vulkan and FourCC format codes.
///
/// Any entry in this table may have attributes associated with a conversion. This is needed for `PACK` Vulkan
/// formats which may only have an alternative given a specific host endian.
///
/// See the module documentation for usage details.
macro_rules! vk_format_table {
    (
        $(
            // This meta specifier is used for format conversions for PACK formats.
            $(#[$conv_meta:meta])*
            $fourcc: ident => $vk: ident
        ),* $(,)?
    ) => {
        /// Converts a FourCC format code to a Vulkan format code.
        ///
        /// This will return [`None`] if the format is not known.
        ///
        /// These format conversions will return all known FourCC and Vulkan format conversions. However a
        /// Vulkan implementation may not support some Vulkan format. One notable example of this are the
        /// formats introduced in `VK_EXT_4444_formats`. The corresponding FourCC codes will return the
        /// formats from `VK_EXT_4444_formats`, but the caller is responsible for testing that a Vulkan device
        /// supports these formats.
        pub fn get_vk_format(fourcc: $crate::backend::allocator::Fourcc) -> Option<ash::vk::Format> {
            // FIXME: Use reexport for ash::vk::Format
            match $crate::backend::allocator::format::get_transparent(fourcc).unwrap_or(fourcc) {
                $(
                    $(#[$conv_meta])*
                    $crate::backend::allocator::Fourcc::$fourcc => Some(ash::vk::Format::$vk),
                )*

                _ => None,
            }
        }

        /// Converts a Vulkan format code to a FourCC format code.
        ///
        /// This will return [`None`] if the format is not known.
        ///
        /// These format conversions will return all known FourCC and Vulkan format conversions. However a
        /// Vulkan implementation may not support some Vulkan format. One notable example of this are the
        /// formats introduced in `VK_EXT_4444_formats`. The corresponding FourCC codes will return the
        /// formats from `VK_EXT_4444_formats`, but the caller is responsible for testing that a Vulkan device
        /// supports these formats.
        pub const fn get_drm_format(vk: ash::vk::Format) -> Option<$crate::backend::allocator::Fourcc> {
            // FIXME: Use reexport for ash::vk::Format
            match vk {
                $(
                    $(#[$conv_meta])*
                    ash::vk::Format::$vk => Some($crate::backend::allocator::Fourcc::$fourcc),
                )*

                _ => None,
            }
        }

        /// Returns all the known format conversions.
        ///
        /// The list contains FourCC format codes that may be converted using [`get_vk_format`].
        pub const fn known_formats() -> &'static [$crate::backend::allocator::Fourcc] {
            &[
                $(
                    $crate::backend::allocator::Fourcc::$fourcc
                ),*
            ]
        }
    };
}

// Vulkan classifies formats by both channel sizes and colorspace. FourCC format codes do not classify formats
// based on colorspace. Additionally the Vulkan SRGB formats correspond to unpremultiplied alpha, but we need premultiplied alpha on electrical values.
//
// As such we are using UNORM/SFLOAT here and handle colorspace separately (once we know how).
vk_format_table! {
    // Vulkan non-packed 8-bits-per-channel formats have an inverted channel
    // order compared to the DRM formats, because DRM format channel order
    // is little-endian while Vulkan format channel order is in memory byte
    // order.

    R8 => R8_UNORM,
    // TODO: Update drm-fourcc
    //R16f => R16_SFLOAT,
    //R32f => R32_SFLOAT,
    Gr88 => R8G8_UNORM,
    // TODO: Update drm-fourcc
    //Gr1616f => R16G16_SFLOAT,
    //Gr3232f => R32G32_SFLOAT,
    Rgb888 => B8G8R8_UNORM,
    Bgr888 => R8G8B8_UNORM,
    Argb8888 => B8G8R8A8_UNORM,
    Abgr8888 => R8G8B8A8_UNORM,

    // PACK32 formats are equivalent to u32 instead of [u8; 4] and thus depend their layout depends the host
    // endian.

    #[cfg(target_endian = "little")]
    Rgba4444 => R4G4B4A4_UNORM_PACK16,
    #[cfg(target_endian = "little")]
    Bgra4444 => B4G4R4A4_UNORM_PACK16,
    #[cfg(target_endian = "little")]
    Rgb565 => R5G6B5_UNORM_PACK16,
    #[cfg(target_endian = "little")]
    Bgr565 => B5G6R5_UNORM_PACK16,
    #[cfg(target_endian = "little")]
    Rgba5551 => R5G5B5A1_UNORM_PACK16,
    #[cfg(target_endian = "little")]
    Bgra5551 => B5G5R5A1_UNORM_PACK16,
    #[cfg(target_endian = "little")]
    Argb1555 => A1R5G5B5_UNORM_PACK16,
    #[cfg(target_endian = "little")]
    Rgba8888 => A8B8G8R8_UNORM_PACK32,
    #[cfg(target_endian = "little")]
    Argb2101010 => A2R10G10B10_UNORM_PACK32,
    #[cfg(target_endian = "little")]
    Abgr2101010 => A2B10G10R10_UNORM_PACK32,

    // Vulkan 16-bits-per-channel formats have an inverted channel order
    // compared to DRM formats, just like the 8-bits-per-channel ones.
    // On little endian systems the memory representation of each channel
    // matches the DRM formats'.

    // TODO: Update drm-fourcc
    //#[cfg(target_endian = "little")]
    //Bgr161616 => R16G16B16_UNORM,
    //#[cfg(target_endian = "little")]
    //Bgr161616f => R16G16B16_SFLOAT,
    //#[cfg(target_endian = "little")]
    //Abgr16161616 => R16G16B16A16_UNORM,
    //#[cfg(target_endian = "little")]
    //Xbgr16161616 => R16G16B16A16_UNORM,
    #[cfg(target_endian = "little")]
    Abgr16161616f => R16G16B16A16_SFLOAT,
    //#[cfg(target_endian = "little")]
    //Bgr323232f => R32G32B32_SFLOAT,
    //#[cfg(target_endian = "little")]
    //Abgr32323232f => R32G32B32A32_SFLOAT,

    // YCbCr formats
    // R -> V, G -> Y, B -> U
    // 420 -> 2x2 subsampled, 422 -> 2x1 subsampled, 444 -> non-subsampled
    Uyvy => B8G8R8G8_422_UNORM,
    Yuyv => G8B8G8R8_422_UNORM,
    Nv12 => G8_B8R8_2PLANE_420_UNORM,
    Nv16 => G8_B8R8_2PLANE_422_UNORM,
    Yuv420 => G8_B8_R8_3PLANE_420_UNORM,
    Yuv422 => G8_B8_R8_3PLANE_422_UNORM,
    Yuv444 => G8_B8_R8_3PLANE_444_UNORM,

    // 3PACK16 formats split the memory in three 16-bit words, so they have an
    // inverted channel order compared to DRM formats.
    #[cfg(target_endian = "little")]
    P010 => G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16,
    #[cfg(target_endian = "little")]
    P210 => G10X6_B10X6R10X6_2PLANE_422_UNORM_3PACK16,
    #[cfg(target_endian = "little")]
    P012 => G12X4_B12X4R12X4_2PLANE_420_UNORM_3PACK16,
    #[cfg(target_endian = "little")]
    P016 => G16_B16R16_2PLANE_420_UNORM,
    #[cfg(target_endian = "little")]
    Q410 => G10X6_B10X6_R10X6_3PLANE_444_UNORM_3PACK16,

    // TODO: add DRM_FORMAT_NV24/VK_FORMAT_G8_B8R8_2PLANE_444_UNORM (requires
    // Vulkan 1.3 or VK_EXT_ycbcr_2plane_444_formats)
}

#[derive(Debug, Clone)]
pub struct FormatEntry {
    pub format: Format,
    pub modifier_properties: vk::DrmFormatModifierPropertiesEXT,
}

pub type FormatList = Vec<FormatEntry>;

pub fn component_mapping_for_format(format: vk::Format, has_alpha: bool) -> vk::ComponentMapping {
    match format {
        vk::Format::B8G8R8_UNORM | vk::Format::B8G8R8A8_UNORM => vk::ComponentMapping {
            r: vk::ComponentSwizzle::B,
            g: vk::ComponentSwizzle::IDENTITY,
            b: vk::ComponentSwizzle::R,
            a: if has_alpha {
                vk::ComponentSwizzle::IDENTITY
            } else {
                vk::ComponentSwizzle::ONE
            },
        },
        #[cfg(target_endian = "little")]
        vk::Format::B4G4R4A4_UNORM_PACK16
        | vk::Format::B5G6R5_UNORM_PACK16
        | vk::Format::B5G5R5A1_UNORM_PACK16 => vk::ComponentMapping {
            r: vk::ComponentSwizzle::B,
            g: vk::ComponentSwizzle::IDENTITY,
            b: vk::ComponentSwizzle::R,
            a: if has_alpha {
                vk::ComponentSwizzle::IDENTITY
            } else {
                vk::ComponentSwizzle::ONE
            },
        },
        #[cfg(target_endian = "little")]
        vk::Format::A1R5G5B5_UNORM_PACK16 | vk::Format::A2R10G10B10_UNORM_PACK32 => vk::ComponentMapping {
            r: vk::ComponentSwizzle::G,
            g: vk::ComponentSwizzle::B,
            b: vk::ComponentSwizzle::A,
            a: vk::ComponentSwizzle::R,
        },
        #[cfg(target_endian = "little")]
        vk::Format::A1B5G5R5_UNORM_PACK16_KHR
        | vk::Format::A2B10G10R10_UNORM_PACK32
        | vk::Format::A8B8G8R8_UNORM_PACK32 => vk::ComponentMapping {
            r: vk::ComponentSwizzle::A,
            g: vk::ComponentSwizzle::B,
            b: vk::ComponentSwizzle::G,
            a: vk::ComponentSwizzle::R,
        },
        _ => vk::ComponentMapping {
            r: vk::ComponentSwizzle::IDENTITY,
            g: vk::ComponentSwizzle::IDENTITY,
            b: vk::ComponentSwizzle::IDENTITY,
            a: if has_alpha {
                vk::ComponentSwizzle::IDENTITY
            } else {
                vk::ComponentSwizzle::ONE
            },
        },
    }
}

impl PhysicalDevice {
    pub fn drm_formats(&self) -> FormatList {
        let mut list = Vec::new();

        for &fourcc in known_formats() {
            let vk_format = get_vk_format(fourcc).unwrap();
            let modifier_properties = self
                .get_format_modifier_properties(vk_format)
                .expect("The Vulkan allocator requires VK_EXT_image_drm_format_modifier");

            for modifier_properties in modifier_properties {
                list.push(FormatEntry {
                    format: Format {
                        code: fourcc,
                        modifier: Modifier::from(modifier_properties.drm_format_modifier),
                    },
                    modifier_properties,
                });
            }
        }

        list
    }

    /// Returns whether the format + modifier combination and the usage flags are supported.
    pub fn drm_format_info(
        &self,
        format: Format,
        usage: vk::ImageUsageFlags,
    ) -> Result<Option<vk::ImageFormatProperties>, vk::Result> {
        let vk_format = get_vk_format(format.code);

        match vk_format {
            Some(vk_format) => {
                let mut image_drm_format_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
                    .drm_format_modifier(format.modifier.into())
                    .sharing_mode(vk::SharingMode::EXCLUSIVE);
                let format_info = vk::PhysicalDeviceImageFormatInfo2::default()
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
                    self.instance()
                        .handle()
                        .get_physical_device_image_format_properties2(
                            self.handle(),
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
}
