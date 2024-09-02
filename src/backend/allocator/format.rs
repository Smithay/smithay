//! Format info tables for DRM formats.
//!
//! This module provides three functions, [`get_opaque`], [`has_alpha`] and [`get_bpp`].
//!
//! [`get_opaque`] returns the opaque alternative of a DRM format with an alpha channel.
//!
//! ```
//! # use smithay::backend::allocator::Fourcc;
//! # use smithay::backend::allocator::format::get_opaque;
//! assert_eq!(Some(Fourcc::Xrgb8888), get_opaque(Fourcc::Argb8888));
//! ```
//!
//! [`has_alpha`] returns true if the format has an alpha channel.
//!
//! ```
//! # use smithay::backend::allocator::Fourcc;
//! # use smithay::backend::allocator::format::has_alpha;
//! assert!(has_alpha(Fourcc::Argb8888));
//! assert!(!has_alpha(Fourcc::Xrgb8888));
//! ```
//!
//! [`get_bpp`] returns the number of bits per pixel of a format.
//!
//! ```
//! # use smithay::backend::allocator::Fourcc;
//! # use smithay::backend::allocator::format::get_bpp;
//! assert_eq!(get_bpp(Fourcc::Argb8888), Some(32));
//! ```
//!
//! [`get_depth`] returns the number of used bits per pixel of a format
//! (excluding padding or non-alpha "X" parts of the format).
//!
//! ```
//! # use smithay::backend::allocator::Fourcc;
//! # use smithay::backend::allocator::format::get_depth;
//! assert_eq!(get_depth(Fourcc::Argb8888), Some(32));
//! assert_eq!(get_depth(Fourcc::Xrgb8888), Some(24));
//! ```

use std::sync::Arc;

use super::Format;
use indexmap::IndexSet;

/// Macro to generate table lookup functions for formats.
///
/// See the module documentation for usage details.
macro_rules! format_tables {
    (
        $($fourcc: ident {
            $(opaque: $opaque: ident,)?
            alpha: $alpha: expr,
            bpp: $bpp: expr,
            depth: $depth: expr $(,)?
        }),*
    ) => {
        /// Returns the opaque alternative of the specified format.
        ///
        /// If the format has an alpha channel, this may return the corresponding opaque format.
        ///
        /// Unknown formats will always return [`None`].
        pub const fn get_opaque(
            fourcc: $crate::backend::allocator::Fourcc,
        ) -> Option<$crate::backend::allocator::Fourcc> {
            match fourcc {
                $($(
                    $crate::backend::allocator::Fourcc::$fourcc
                        => Some($crate::backend::allocator::Fourcc::$opaque),
                )?)*
                _ => None,
            }
        }

        /// Returns the transparent alternative of the specified format.
        ///
        /// If the format has an unused alpha channel, this may return the corresponding non-opaque format.
        ///
        /// Unknown formats will always return [`None`].
        pub const fn get_transparent(
            fourcc: $crate::backend::allocator::Fourcc,
        ) -> Option<$crate::backend::allocator::Fourcc> {
            match fourcc {
                $($(
                    $crate::backend::allocator::Fourcc::$opaque
                        => Some($crate::backend::allocator::Fourcc::$fourcc),
                )?)*
                _ => None,
            }
        }

        /// Returns true if the format has an alpha channel.
        ///
        /// This function may be useful to know if the alpha channel may need to be swizzled when rendering
        /// with some graphics apis.
        ///
        /// Unknown formats will always return `false`.
        pub const fn has_alpha(fourcc: $crate::backend::allocator::Fourcc) -> bool {
            match fourcc {
                $(
                    $crate::backend::allocator::Fourcc::$fourcc => $alpha,
                )*
                _ => false,
            }
        }

        /// Returns the bits per pixel of the specified format.
        ///
        /// Unknown formats will always return [`None`].
        pub const fn get_bpp(
            fourcc: $crate::backend::allocator::Fourcc,
        ) -> Option<usize> {
            match fourcc {
                $($crate::backend::allocator::Fourcc::$fourcc => Some($bpp),)*
                _ => None,
            }
        }

        /// Returns the depth of the specified format.
        ///
        /// Unknown formats will always return [`None`].
        pub const fn get_depth(
            fourcc: $crate::backend::allocator::Fourcc,
        ) -> Option<usize> {
            match fourcc {
                $($crate::backend::allocator::Fourcc::$fourcc => Some($depth),)*
                _ => None,
            }
        }

        fn _impl_formats() -> &'static [$crate::backend::allocator::Fourcc] {
            &[
                $(
                    $crate::backend::allocator::Fourcc::$fourcc,
                )*
            ]
        }
    };
}

format_tables! {
    // 8-bit bpp Red
    R8 { alpha: false, bpp: 8, depth: 8 },

    // TODO: Update drm-fourcc
    // 16-bit bpp Red with padding (x:R)
    // R10 { bpp: 16, depth: 10 },
    // R12 { bpp: 16, depth: 12 },

    // 16-bit bpp Red
    R16 { alpha: false, bpp: 16, depth: 16 },

    // 16-bit bpp RG
    Rg88 { alpha: false, bpp: 16, depth: 16 },

    Gr88 { alpha: false, bpp: 16, depth: 16 },

    // 32-bit bpp RG
    Rg1616 { alpha: false, bpp: 32, depth: 32 },

    Gr1616 { alpha: false, bpp: 32, depth: 32 },

    // 8-bit bpp RGB
    Rgb332 { alpha: false, bpp: 8, depth: 8 },

    Bgr233 { alpha: false, bpp: 8, depth: 8 },

    // 16-bit bpp RGB, 4 bits per channel
    Argb4444 {
        opaque: Xrgb4444,
        alpha: true,
        bpp: 16,
        depth: 16,
    },

    Xrgb4444 { alpha: false, bpp: 16, depth: 12 },

    Abgr4444 {
        opaque: Xbgr4444,
        alpha: true,
        bpp: 16,
        depth: 16,
    },

    Xbgr4444 { alpha: false, bpp: 16, depth: 12 },

    Rgba4444 {
        opaque: Rgbx4444,
        alpha: true,
        bpp: 16,
        depth: 16,
    },

    Rgbx4444 { alpha: false, bpp: 16, depth: 12 },

    Bgra4444 {
        opaque: Bgrx4444,
        alpha: true,
        bpp: 16,
        depth: 16,
    },

    Bgrx4444 { alpha: false, bpp: 16, depth: 12 },

    // 16-bit bpp RGB, 5 bits per color channel, 1 bit for alpha channel
    Argb1555 {
        opaque: Xrgb1555,
        alpha: true,
        bpp: 16,
        depth: 16
    },

    Xrgb1555 { alpha: false, bpp: 16, depth: 15 },

    Abgr1555 {
        opaque: Xbgr1555,
        alpha: true,
        bpp: 16,
        depth: 16,
    },

    Xbgr1555 { alpha: false, bpp: 16, depth: 15 },

    Rgba5551 {
        opaque: Rgbx5551,
        alpha: true,
        bpp: 16,
        depth: 16,
    },

    Rgbx5551 { alpha: false, bpp: 16, depth: 15 },

    Bgra5551 {
        opaque: Bgrx5551,
        alpha: true,
        bpp: 16,
        depth: 16,
    },

    Bgrx5551 { alpha: false, bpp: 16, depth: 15 },

    // 16-bit bpp RGB, no alpha, 6 bits for green channel and 5 bits for blue and red
    Rgb565 { alpha: false, bpp: 16, depth: 16 },

    Bgr565 { alpha: false, bpp: 16, depth: 16 },

    // 24-bit bpp RGB
    Rgb888 { alpha: false, bpp: 24, depth: 24 },

    Bgr888 { alpha: false, bpp: 24, depth: 24 },

    // 32-bit bpp RGB, 8 bits per channel
    Argb8888 {
        opaque: Xrgb8888,
        alpha: true,
        bpp: 32,
        depth: 32,
    },

    Xrgb8888 { alpha: false, bpp: 32, depth: 24 },

    Abgr8888 {
        opaque: Xbgr8888,
        alpha: true,
        bpp: 32,
        depth: 32,
    },

    Xbgr8888 { alpha: false, bpp: 32, depth: 24 },

    Rgba8888 {
        opaque: Rgbx8888,
        alpha: true,
        bpp: 32,
        depth: 32,
    },

    Rgbx8888 { alpha: false, bpp: 32, depth: 24 },

    Bgra8888 {
        opaque: Bgrx8888,
        alpha: true,
        bpp: 32,
        depth: 32,
    },

    Bgrx8888 { alpha: false, bpp: 32, depth: 24 },

    // 32-bit bpp RGB with 10-bits per color channel

    Argb2101010 {
        opaque: Xrgb2101010,
        alpha: true,
        bpp: 32,
        depth: 32,
    },

    Xrgb2101010 { alpha: false, bpp: 32, depth: 30 },

    Abgr2101010 {
        opaque: Xbgr2101010,
        alpha: true,
        bpp: 32,
        depth: 32,
    },

    Xbgr2101010 { alpha: false, bpp: 32, depth: 30 },

    Rgba1010102 {
        opaque: Rgbx1010102,
        alpha: true,
        bpp: 32,
        depth: 32,
    },

    Rgbx1010102 { alpha: false, bpp: 32, depth: 30 },

    Bgra1010102 {
        opaque: Bgrx1010102,
        alpha: true,
        bpp: 32,
        depth: 32,
    },

    Bgrx1010102 { alpha: false, bpp: 32, depth: 30 },

    // 64-bit RGB, 16-bits per channel
    // TODO: Update drm-fourcc
    // Argb16161616 {
    //     opaque: Xrgb16161616,
    //     alpha: true,
    //     bpp: 64,
    //     depth: 64,
    // },
    // Xrgb16161616 { alpha: false, bpp: 64, depth: 48 },
    // Abgr16161616 {
    //     opaque: Xbgr16161616,
    //     alpha: true,
    //     bpp: 64,
    //     depth: 64,
    // },
    // Xbgr16161616 { alpha: false, bpp: 64, depth: 48 },

    // Floating point 64bpp RGB
    // IEEE 754-2008 binary16 half-precision float
    Argb16161616f {
        opaque: Xrgb16161616f,
        alpha: true,
        bpp: 64,
        depth: 64
    },

    Xrgb16161616f { alpha: false, bpp: 64, depth: 48 },

    Abgr16161616f {
        opaque: Xbgr16161616f,
        alpha: true,
        bpp: 64,
        depth: 64,
    },

    Xbgr16161616f { alpha: false, bpp: 64, depth: 48 },

    // RGBA with 10-bit components packed in 64-bits per pixel, with 6-bits of unused padding per component

    // Axbxgxrx106106106106 has no direct non-alpha alternative.
    Axbxgxrx106106106106 { alpha: true, bpp: 64, depth: 40 }

    // TODO: YUV and other formats
}

/// A set of [`Format`]s
#[derive(Debug, Default, Clone)]
pub struct FormatSet {
    formats: Arc<IndexSet<Format>>,
}

impl FormatSet {
    #[cfg(any(feature = "backend_egl", feature = "backend_drm"))]
    pub(crate) fn from_formats(formats: IndexSet<Format>) -> Self {
        FormatSet {
            formats: Arc::new(formats),
        }
    }
}

impl FormatSet {
    /// Return an iterator over the values of the set, in their order
    pub fn iter(&self) -> FormatSetIter<'_> {
        FormatSetIter {
            inner: self.formats.iter(),
        }
    }

    /// Return `true` if an equivalent to `value` exists in the set.
    pub fn contains(&self, format: &Format) -> bool {
        self.formats.contains(format)
    }

    /// Return an iterator over the values that are in both `self` and `other`.
    pub fn intersection<'a>(&'a self, other: &'a FormatSet) -> FormatSetIntersection<'a> {
        FormatSetIntersection {
            inner: self.formats.intersection(&other.formats),
        }
    }

    /// Get access to the underlying storage.
    pub fn indexset(&self) -> &IndexSet<Format> {
        &self.formats
    }
}

/// A lazy iterator producing elements in the intersection of [`FormatSet`]s.
#[derive(Debug)]
pub struct FormatSetIntersection<'a> {
    inner: indexmap::set::Intersection<'a, Format, std::collections::hash_map::RandomState>,
}

impl<'a> Iterator for FormatSetIntersection<'a> {
    type Item = &'a Format;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl IntoIterator for FormatSet {
    type Item = Format;

    type IntoIter = FormatSetIntoIter;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        FormatSetIntoIter {
            inner: (*self.formats).clone().into_iter(),
        }
    }
}

impl FromIterator<Format> for FormatSet {
    #[inline]
    fn from_iter<T: IntoIterator<Item = Format>>(iter: T) -> Self {
        Self {
            formats: Arc::new(IndexSet::from_iter(iter)),
        }
    }
}

/// An iterator over the items of an [`FormatSet`].
#[derive(Debug)]
pub struct FormatSetIter<'a> {
    inner: indexmap::set::Iter<'a, Format>,
}

impl<'a> Iterator for FormatSetIter<'a> {
    type Item = &'a Format;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

/// An owning iterator over the items of an [`FormatSet`].
#[derive(Debug)]
pub struct FormatSetIntoIter {
    inner: indexmap::set::IntoIter<Format>,
}

impl Iterator for FormatSetIntoIter {
    type Item = Format;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::{_impl_formats, get_bpp, get_depth, get_opaque, get_transparent, has_alpha};

    /// Tests that opaque alternatives are not the same as the variant with alpha.
    #[test]
    fn opaque_neq() {
        for &format in _impl_formats() {
            if let Some(opaque) = get_opaque(format) {
                assert_ne!(
                    format, opaque,
                    "{}'s opaque alternative is the same format",
                    format
                );
            }
        }
    }

    /// Tests that opaque alternatives are cleanly converting back with get_transparent.
    #[test]
    fn opaque_inverse() {
        for &format in _impl_formats() {
            if let Some(opaque) = get_opaque(format) {
                let transparent = get_transparent(opaque);
                assert_eq!(
                    Some(format),
                    transparent,
                    "{}'s opaque alternative {} doesn't cleanly convert back, got: {:?}",
                    format,
                    opaque,
                    transparent
                );
            }
        }
    }

    /// Tests that opaque alternatives for formats do not have opaque alternatives themselves.
    ///
    /// For example, `Argb8888` should have an opaque alternative of `Xrgb8888` and `Xrgb8888` should have no
    /// opaque alternative.
    #[test]
    fn opaque_alternatives() {
        for &format in _impl_formats() {
            if let Some(opaque) = get_opaque(format) {
                // If a format is considered to be an opaque alternative, the format should not have an opaque
                // alternative.
                let result = get_opaque(opaque);

                assert!(
                    result.is_none(),
                    "Format {format} has an opaque alternative, {opaque}. However {opaque} reports an opaque alternative {opaque_opaque:?} which is incorrect",
                    format = format,
                    opaque = opaque,
                    opaque_opaque = result,
                );
            }
        }
    }

    /// Tests that a format and it's opaque alternative have the same number of bits per pixel.
    #[test]
    fn opaque_has_same_bpp() {
        for &format in _impl_formats() {
            if let Some(opaque) = get_opaque(format) {
                let format_bpp = get_bpp(format);
                let opaque_bpp = get_bpp(opaque);

                assert_eq!(
                    format_bpp,
                    opaque_bpp,
                    "Format {format} has a bpp of {format_bpp:?}. However the opaque alternative {opaque} has a different bpp of {opaque_bpp:?}",
                    format_bpp = get_bpp(format),
                    opaque = opaque,
                    opaque_bpp = get_bpp(opaque),
                );
            }
        }
    }

    /// A format with an opaque alternative should have alpha.
    ///
    /// The opaque alternative should not have alpha.
    #[test]
    fn format_with_opaque_has_alpha() {
        for &format in _impl_formats() {
            if let Some(opaque) = get_opaque(format) {
                // Since the format has an opaque alternative, verify the opaque alternative does not have alpha.
                assert!(
                    has_alpha(format),
                    "{} has an opaque alternative but does not state it has an alpha component",
                    format
                );

                // The opaque alternative should not have alpha.
                assert!(
                    !has_alpha(opaque),
                    "opaque alternative to {} ({}) has an alpha channel",
                    format,
                    opaque
                );
            }
        }
    }

    // A format's depth should always be equal or small to it's bits-per-pixel
    #[test]
    fn format_bpp_greater_or_equal_than_depth() {
        for &format in _impl_formats() {
            assert!(
                get_depth(format) <= get_bpp(format),
                "{} has a depth higher than its bpp",
                format
            );
        }
    }
}
