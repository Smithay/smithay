//! Format conversions between Vulkan and DRM formats.

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
        pub const fn get_vk_format(fourcc: $crate::backend::allocator::Fourcc) -> Option<ash::vk::Format> {
            // FIXME: Use reexport for ash::vk::Format
            match fourcc {
                $(
                    $(#[$conv_meta])*
                    $crate::backend::allocator::Fourcc::$fourcc => Some(ash::vk::Format::$vk),
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

// FIXME: SRGB format is not always correct.
//
// Vulkan classifies formats by both channel sizes and colorspace. FourCC format codes do not classify formats
// based on colorspace.
//
// To implement this correctly, it is likely that parsing vulkan.xml and classifying families of colorspaces
// would be needed since there are a lot of formats.
//
// Many of these conversions come from wsi_common_wayland.c in Mesa
vk_format_table! {
    Argb8888 => B8G8R8A8_SRGB,
    Xrgb8888 => B8G8R8A8_SRGB,

    Abgr8888 => R8G8B8A8_SRGB,
    Xbgr8888 => R8G8B8A8_SRGB,

    // PACK32 formats are equivalent to u32 instead of [u8; 4] and thus depend their layout depends the host
    // endian.
    #[cfg(target_endian = "little")]
    Rgba8888 => A8B8G8R8_SRGB_PACK32,
    #[cfg(target_endian = "little")]
    Rgbx8888 => A8B8G8R8_SRGB_PACK32,

    #[cfg(target_endian = "little")]
    Argb2101010 => A2R10G10B10_UNORM_PACK32,
    #[cfg(target_endian = "little")]
    Xrgb2101010 => A2R10G10B10_UNORM_PACK32,

    #[cfg(target_endian = "little")]
    Abgr2101010 => A2B10G10R10_UNORM_PACK32,
    #[cfg(target_endian = "little")]
    Xbgr2101010 => A2B10G10R10_UNORM_PACK32,
}
