use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::wl_output::Transform as WlTransform;

/// Type-level marker for the logical coordinate space
#[derive(Debug)]
pub struct Logical;

/// Type-level marker for the physical coordinate space
#[derive(Debug)]
pub struct Physical;

/// Type-level marker for the buffer coordinate space
#[derive(Debug)]
pub struct Buffer;

/// Type-level marker for raw coordinate space, provided by input devices
#[derive(Debug)]
pub struct Raw;

/// Trait for types serving as a coordinate for other geometry utils
pub trait Coordinate:
    Sized + Add<Self, Output = Self> + Sub<Self, Output = Self> + PartialOrd + Default + Copy + fmt::Debug
{
    /// Downscale the coordinate
    fn downscale(self, scale: Self) -> Self;
    /// Upscale the coordinate
    fn upscale(self, scale: Self) -> Self;
    /// Convert the coordinate to a f64
    fn to_f64(self) -> f64;
    /// Convert to this coordinate from a f64
    fn from_f64(v: f64) -> Self;
    /// Compare and return the smaller one
    fn min(self, other: Self) -> Self {
        if self < other {
            self
        } else {
            other
        }
    }
    /// Compare and return the larger one
    fn max(self, other: Self) -> Self {
        if self > other {
            self
        } else {
            other
        }
    }
    /// Test if the coordinate is not negative
    fn non_negative(self) -> bool;
    /// Returns the absolute value of this coordinate
    fn abs(self) -> Self;

    /// Saturating integer addition. Computes self + other, saturating at the numeric bounds instead of overflowing.
    fn saturating_add(self, other: Self) -> Self;
    /// Saturating integer subtraction. Computes self - other, saturating at the numeric bounds instead of overflowing.
    fn saturating_sub(self, other: Self) -> Self;
    /// Saturating integer multiplication. Computes self * other, saturating at the numeric bounds instead of overflowing.
    fn saturating_mul(self, other: Self) -> Self;
}

/// Implements Coordinate for an unsigned numerical type.
macro_rules! unsigned_coordinate_impl {
    ($ty:ty, $ ($tys:ty),* ) => {
        unsigned_coordinate_impl!($ty);
        $(
            unsigned_coordinate_impl!($tys);
        )*
    };

    ($ty:ty) => {
        impl Coordinate for $ty {
            #[inline]
            fn downscale(self, scale: Self) -> Self {
                self / scale
            }

            #[inline]
            fn upscale(self, scale: Self) -> Self {
                self.saturating_mul(scale)
            }

            #[inline]
            fn to_f64(self) -> f64 {
                self as f64
            }

            #[inline]
            fn from_f64(v: f64) -> Self {
                v as Self
            }

            #[inline]
            fn non_negative(self) -> bool {
                true
            }

            #[inline]
            fn abs(self) -> Self {
                self
            }

            #[inline]
            fn saturating_add(self, other: Self) -> Self {
                self.saturating_add(other)
            }
            #[inline]
            fn saturating_sub(self, other: Self) -> Self {
                self.saturating_sub(other)
            }
            #[inline]
            fn saturating_mul(self, other: Self) -> Self {
                self.saturating_mul(other)
            }
        }
    };
}

unsigned_coordinate_impl! {
    u8,
    u16,
    u32,
    u64,
    u128
}

/// Implements Coordinate for an signed numerical type.
macro_rules! signed_coordinate_impl {
    ($ty:ty, $ ($tys:ty),* ) => {
        signed_coordinate_impl!($ty);
        $(
            signed_coordinate_impl!($tys);
        )*
    };

    ($ty:ty) => {
        impl Coordinate for $ty {
            #[inline]
            fn downscale(self, scale: Self) -> Self {
                self / scale
            }

            #[inline]
            fn upscale(self, scale: Self) -> Self {
                self.saturating_mul(scale)
            }

            #[inline]
            fn to_f64(self) -> f64 {
                self as f64
            }

            #[inline]
            fn from_f64(v: f64) -> Self {
                v as Self
            }

            #[inline]
            fn non_negative(self) -> bool {
                self >= 0
            }

            #[inline]
            fn abs(self) -> Self {
                self.abs()
            }

            #[inline]
            fn saturating_add(self, other: Self) -> Self {
                self.saturating_add(other)
            }
            #[inline]
            fn saturating_sub(self, other: Self) -> Self {
                self.saturating_sub(other)
            }
            #[inline]
            fn saturating_mul(self, other: Self) -> Self {
                self.saturating_mul(other)
            }
        }
    };
}

signed_coordinate_impl! {
    i8,
    i16,
    i32,
    i64,
    i128
}

macro_rules! floating_point_coordinate_impl {
    ($ty:ty, $ ($tys:ty),* ) => {
        floating_point_coordinate_impl!($ty);
        $(
            floating_point_coordinate_impl!($tys);
        )*
    };

    ($ty:ty) => {
        impl Coordinate for $ty {
            #[inline]
            fn downscale(self, scale: Self) -> Self {
                self / scale
            }

            #[inline]
            fn upscale(self, scale: Self) -> Self {
                self * scale
            }

            #[inline]
            fn to_f64(self) -> f64 {
                self as f64
            }

            #[inline]
            fn from_f64(v: f64) -> Self {
                v as Self
            }

            #[inline]
            fn non_negative(self) -> bool {
                self >= 0.0
            }

            #[inline]
            fn abs(self) -> Self {
                self.abs()
            }

            #[inline]
            fn saturating_add(self, other: Self) -> Self {
                self + other
            }
            #[inline]
            fn saturating_sub(self, other: Self) -> Self {
                self - other
            }
            #[inline]
            fn saturating_mul(self, other: Self) -> Self {
                self * other
            }
        }
    };
}

floating_point_coordinate_impl! {
    f32,
    f64
}

/*
 * Point
 */

/// A point as defined by its x and y coordinates
///
/// Operations on points are saturating.
#[repr(C)]
pub struct Point<N, Kind> {
    /// horizontal coordinate
    pub x: N,
    /// vertical coordinate
    pub y: N,
    _kind: std::marker::PhantomData<Kind>,
}

impl<N: Coordinate, Kind> Point<N, Kind> {
    /// Convert this [`Point`] to a [`Size`] with the same coordinates
    ///
    /// Checks that the coordinates are positive with a `debug_assert!()`.
    #[inline]
    pub fn to_size(self) -> Size<N, Kind> {
        debug_assert!(
            self.x.non_negative() && self.y.non_negative(),
            "Attempting to create a `Size` of negative size: {:?}",
            (self.x, self.y)
        );
        Size {
            w: self.x,
            h: self.y,
            _kind: std::marker::PhantomData,
        }
    }

    /// Convert this [`Point`] to a [`Size`] with the same coordinates
    ///
    /// Ensures that the coordinates are positive by taking their absolute value
    #[inline]
    pub fn to_size_abs(self) -> Size<N, Kind> {
        Size {
            w: self.x.abs(),
            h: self.y.abs(),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate, Kind> Point<N, Kind> {
    /// Constrain this [`Point`] within a [`Rectangle`] with the same coordinates
    ///
    /// The [`Point`] returned is guaranteed to be not smaller than the [`Rectangle`]
    /// location and not greater than the [`Rectangle`] size.
    #[inline]
    pub fn constrain(self, rect: impl Into<Rectangle<N, Kind>>) -> Point<N, Kind> {
        let rect = rect.into();

        Point {
            x: self.x.max(rect.loc.x).min(rect.size.w),
            y: self.y.max(rect.loc.y).min(rect.size.h),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate, Kind> Point<N, Kind> {
    /// Convert the underlying numerical type to f64 for floating point manipulations
    #[inline]
    pub fn to_f64(self) -> Point<f64, Kind> {
        Point {
            x: self.x.to_f64(),
            y: self.y.to_f64(),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<Kind> Point<f64, Kind> {
    /// Convert to i32 for integer-space manipulations by rounding float values
    #[inline]
    pub fn to_i32_round<N: Coordinate>(self) -> Point<N, Kind> {
        Point {
            x: N::from_f64(self.x.round()),
            y: N::from_f64(self.y.round()),
            _kind: std::marker::PhantomData,
        }
    }

    /// Convert to i32 for integer-space manipulations by flooring float values
    #[inline]
    pub fn to_i32_floor<N: Coordinate>(self) -> Point<N, Kind> {
        Point {
            x: N::from_f64(self.x.floor()),
            y: N::from_f64(self.y.floor()),
            _kind: std::marker::PhantomData,
        }
    }

    /// Convert to i32 for integer-space manipulations by ceiling float values
    #[inline]
    pub fn to_i32_ceil<N: Coordinate>(self) -> Point<N, Kind> {
        Point {
            x: N::from_f64(self.x.ceil()),
            y: N::from_f64(self.y.ceil()),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: fmt::Debug> fmt::Debug for Point<N, Logical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Point<Logical>")
            .field("x", &self.x)
            .field("y", &self.y)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for Point<N, Physical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Point<Physical>")
            .field("x", &self.x)
            .field("y", &self.y)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for Point<N, Raw> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Point<Raw>")
            .field("x", &self.x)
            .field("y", &self.y)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for Point<N, Buffer> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Point<Buffer>")
            .field("x", &self.x)
            .field("y", &self.y)
            .finish()
    }
}

impl<N: Coordinate> Point<N, Logical> {
    #[inline]
    /// Convert this logical point to physical coordinate space according to given scale factor
    pub fn to_physical(self, scale: N) -> Point<N, Physical> {
        Point {
            x: self.x.upscale(scale),
            y: self.y.upscale(scale),
            _kind: std::marker::PhantomData,
        }
    }

    #[inline]
    /// Convert this logical point to buffer coordinate space according to given scale factor
    pub fn to_buffer(self, scale: N, transformation: Transform, area: &Size<N, Logical>) -> Point<N, Buffer> {
        let point = transformation.transform_point_in(self, area);
        Point {
            x: point.x.upscale(scale),
            y: point.y.upscale(scale),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate> Point<N, Physical> {
    #[inline]
    /// Convert this physical point to logical coordinate space according to given scale factor
    pub fn to_logical(self, scale: N) -> Point<N, Logical> {
        Point {
            x: self.x.downscale(scale),
            y: self.y.downscale(scale),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate> Point<N, Buffer> {
    #[inline]
    /// Convert this physical point to logical coordinate space according to given scale factor
    pub fn to_logical(self, scale: N, transform: Transform, area: &Size<N, Buffer>) -> Point<N, Logical> {
        let point = transform.invert().transform_point_in(self, area);
        Point {
            x: point.x.downscale(scale),
            y: point.y.downscale(scale),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N, Kind> From<(N, N)> for Point<N, Kind> {
    #[inline]
    fn from((x, y): (N, N)) -> Point<N, Kind> {
        Point {
            x,
            y,
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N, Kind> From<Point<N, Kind>> for (N, N) {
    #[inline]
    fn from(point: Point<N, Kind>) -> (N, N) {
        (point.x, point.y)
    }
}

impl<N: Coordinate, Kind> Add for Point<N, Kind> {
    type Output = Point<N, Kind>;
    #[inline]
    fn add(self, other: Point<N, Kind>) -> Point<N, Kind> {
        Point {
            x: self.x.saturating_add(other.x),
            y: self.y.saturating_add(other.y),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate, Kind> AddAssign for Point<N, Kind> {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.x = self.x.saturating_add(rhs.x);
        self.y = self.y.saturating_add(rhs.y);
    }
}

impl<N: Coordinate, Kind> SubAssign for Point<N, Kind> {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.x = self.x.saturating_sub(rhs.x);
        self.y = self.y.saturating_sub(rhs.y);
    }
}

impl<N: Coordinate, Kind> Sub for Point<N, Kind> {
    type Output = Point<N, Kind>;
    #[inline]
    fn sub(self, other: Point<N, Kind>) -> Point<N, Kind> {
        Point {
            x: self.x.saturating_sub(other.x),
            y: self.y.saturating_sub(other.y),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Clone, Kind> Clone for Point<N, Kind> {
    #[inline]
    fn clone(&self) -> Self {
        Point {
            x: self.x.clone(),
            y: self.y.clone(),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Copy, Kind> Copy for Point<N, Kind> {}

impl<N: PartialEq, Kind> PartialEq for Point<N, Kind> {
    fn eq(&self, other: &Self) -> bool {
        self.x == other.x && self.y == other.y
    }
}

impl<N: Eq, Kind> Eq for Point<N, Kind> {}

impl<N: Default, Kind> Default for Point<N, Kind> {
    fn default() -> Self {
        Point {
            x: N::default(),
            y: N::default(),
            _kind: std::marker::PhantomData,
        }
    }
}

/*
 * Size
 */

/// A size as defined by its width and height
///
/// Constructors of this type ensure that the values are always positive via
/// `debug_assert!()`, however manually changing the values of the fields
/// can break this invariant.
///
/// Operations on sizes are saturating.
#[repr(C)]
pub struct Size<N, Kind> {
    /// horizontal coordinate
    pub w: N,
    /// vertical coordinate
    pub h: N,
    _kind: std::marker::PhantomData<Kind>,
}

impl<N: Coordinate, Kind> Size<N, Kind> {
    /// Convert this [`Size`] to a [`Point`] with the same coordinates
    #[inline]
    pub fn to_point(self) -> Point<N, Kind> {
        Point {
            x: self.w,
            y: self.h,
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate, Kind> Size<N, Kind> {
    /// Restrict this [`Size`] to min and max [`Size`] with the same coordinates
    pub fn clamp(self, min: impl Into<Size<N, Kind>>, max: impl Into<Size<N, Kind>>) -> Size<N, Kind> {
        let min = min.into();
        let max = max.into();

        Size {
            w: self.w.max(min.w).min(max.w),
            h: self.h.max(min.h).min(max.h),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate, Kind> Size<N, Kind> {
    /// Convert the underlying numerical type to f64 for floating point manipulations
    #[inline]
    pub fn to_f64(self) -> Size<f64, Kind> {
        Size {
            w: self.w.to_f64(),
            h: self.h.to_f64(),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<Kind> Size<f64, Kind> {
    /// Convert to i32 for integer-space manipulations by rounding float values
    #[inline]
    pub fn to_i32_round<N: Coordinate>(self) -> Size<N, Kind> {
        Size {
            w: N::from_f64(self.w.round()),
            h: N::from_f64(self.h.round()),
            _kind: std::marker::PhantomData,
        }
    }

    /// Convert to i32 for integer-space manipulations by flooring float values
    #[inline]
    pub fn to_i32_floor<N: Coordinate>(self) -> Size<N, Kind> {
        Size {
            w: N::from_f64(self.w.floor()),
            h: N::from_f64(self.h.floor()),
            _kind: std::marker::PhantomData,
        }
    }

    /// Convert to i32 for integer-space manipulations by ceiling float values
    #[inline]
    pub fn to_i32_ceil<N: Coordinate>(self) -> Size<N, Kind> {
        Size {
            w: N::from_f64(self.w.ceil()),
            h: N::from_f64(self.h.ceil()),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: fmt::Debug> fmt::Debug for Size<N, Logical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Size<Logical>")
            .field("w", &self.w)
            .field("h", &self.h)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for Size<N, Physical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Size<Physical>")
            .field("w", &self.w)
            .field("h", &self.h)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for Size<N, Raw> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Size<Raw>")
            .field("w", &self.w)
            .field("h", &self.h)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for Size<N, Buffer> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Size<Buffer>")
            .field("w", &self.w)
            .field("h", &self.h)
            .finish()
    }
}

impl<N: Coordinate> Size<N, Logical> {
    #[inline]
    /// Convert this logical size to physical coordinate space according to given scale factor
    pub fn to_physical(self, scale: N) -> Size<N, Physical> {
        Size {
            w: self.w.upscale(scale),
            h: self.h.upscale(scale),
            _kind: std::marker::PhantomData,
        }
    }

    #[inline]
    /// Convert this logical size to buffer coordinate space according to given scale factor
    pub fn to_buffer(self, scale: N, transformation: Transform) -> Size<N, Buffer> {
        transformation.transform_size(Size {
            w: self.w.upscale(scale),
            h: self.h.upscale(scale),
            _kind: std::marker::PhantomData,
        })
    }
}

impl<N: Coordinate> Size<N, Physical> {
    #[inline]
    /// Convert this physical point to logical coordinate space according to given scale factor
    pub fn to_logical(self, scale: N) -> Size<N, Logical> {
        Size {
            w: self.w.downscale(scale),
            h: self.h.downscale(scale),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate> Size<N, Buffer> {
    #[inline]
    /// Convert this physical point to logical coordinate space according to given scale factor
    pub fn to_logical(self, scale: N, transformation: Transform) -> Size<N, Logical> {
        transformation.invert().transform_size(Size {
            w: self.w.downscale(scale),
            h: self.h.downscale(scale),
            _kind: std::marker::PhantomData,
        })
    }
}

impl<N: Coordinate, Kind> From<(N, N)> for Size<N, Kind> {
    #[inline]
    fn from((w, h): (N, N)) -> Size<N, Kind> {
        debug_assert!(
            w.non_negative() && h.non_negative(),
            "Attempting to create a `Size` of negative size: {:?}",
            (w, h)
        );
        Size {
            w,
            h,
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N, Kind> From<Size<N, Kind>> for (N, N) {
    #[inline]
    fn from(point: Size<N, Kind>) -> (N, N) {
        (point.w, point.h)
    }
}

impl<N: Coordinate, Kind> Add for Size<N, Kind> {
    type Output = Size<N, Kind>;
    #[inline]
    fn add(self, other: Size<N, Kind>) -> Size<N, Kind> {
        Size {
            w: self.w.saturating_add(other.w),
            h: self.h.saturating_add(other.h),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate, Kind> AddAssign for Size<N, Kind> {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.w = self.w.saturating_add(rhs.w);
        self.h = self.h.saturating_add(rhs.h);
    }
}

impl<N: Coordinate, Kind> SubAssign for Size<N, Kind> {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        debug_assert!(
            self.w >= rhs.w && self.h >= rhs.h,
            "Attempting to subtract bigger from smaller size: {:?} - {:?}",
            (&self.w, &self.h),
            (&rhs.w, &rhs.h),
        );

        self.w = self.w.saturating_sub(rhs.w);
        self.h = self.h.saturating_sub(rhs.h);
    }
}

impl<N: Clone, Kind> Clone for Size<N, Kind> {
    #[inline]
    fn clone(&self) -> Self {
        Size {
            w: self.w.clone(),
            h: self.h.clone(),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Copy, Kind> Copy for Size<N, Kind> {}

impl<N: PartialEq, Kind> PartialEq for Size<N, Kind> {
    fn eq(&self, other: &Self) -> bool {
        self.w == other.w && self.h == other.h
    }
}

impl<N: Eq, Kind> Eq for Size<N, Kind> {}

impl<N: Default, Kind> Default for Size<N, Kind> {
    fn default() -> Self {
        Size {
            w: N::default(),
            h: N::default(),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate, Kind> Add<Size<N, Kind>> for Point<N, Kind> {
    type Output = Point<N, Kind>;
    #[inline]
    fn add(self, other: Size<N, Kind>) -> Point<N, Kind> {
        Point {
            x: self.x.saturating_add(other.w),
            y: self.y.saturating_add(other.h),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate, Kind> Sub<Size<N, Kind>> for Point<N, Kind> {
    type Output = Point<N, Kind>;
    #[inline]
    fn sub(self, other: Size<N, Kind>) -> Point<N, Kind> {
        Point {
            x: self.x.saturating_sub(other.w),
            y: self.y.saturating_sub(other.h),
            _kind: std::marker::PhantomData,
        }
    }
}

/// A rectangle defined by its top-left corner and dimensions
///
/// Operations on retangles are saturating.
#[repr(C)]
pub struct Rectangle<N, Kind> {
    /// Location of the top-left corner of the rectangle
    pub loc: Point<N, Kind>,
    /// Size of the rectangle, as (width, height)
    pub size: Size<N, Kind>,
}

impl<N: Coordinate, Kind> Rectangle<N, Kind> {
    /// Convert the underlying numerical type to another
    pub fn to_f64(self) -> Rectangle<f64, Kind> {
        Rectangle {
            loc: self.loc.to_f64(),
            size: self.size.to_f64(),
        }
    }
}

impl<Kind> Rectangle<f64, Kind> {
    /// Convert to i32 for integer-space manipulations by rounding float values
    #[inline]
    pub fn to_i32_round<N: Coordinate>(self) -> Rectangle<N, Kind> {
        Rectangle {
            loc: self.loc.to_i32_round(),
            size: self.size.to_i32_round(),
        }
    }

    /// Convert to i32 by returning the largest integer-space rectangle fitting into the float-based rectangle
    #[inline]
    pub fn to_i32_down<N: Coordinate>(self) -> Rectangle<N, Kind> {
        Rectangle::from_extemities(self.loc.to_i32_ceil(), (self.loc + self.size).to_i32_floor())
    }

    /// Convert to i32 by returning the smallest integet-space rectangle encapsulating the float-based rectangle
    #[inline]
    pub fn to_i32_up<N: Coordinate>(self) -> Rectangle<N, Kind> {
        Rectangle::from_extemities(self.loc.to_i32_floor(), (self.loc + self.size).to_i32_ceil())
    }
}

impl<N: Coordinate, Kind> Rectangle<N, Kind> {
    /// Create a new [`Rectangle`] from the coordinates of its top-left corner and its dimensions
    #[inline]
    pub fn from_loc_and_size(loc: impl Into<Point<N, Kind>>, size: impl Into<Size<N, Kind>>) -> Self {
        Rectangle {
            loc: loc.into(),
            size: size.into(),
        }
    }

    /// Create a new [`Rectangle`] from the coordinates of its top-left corner and its bottom-right corner
    #[inline]
    pub fn from_extemities(
        topleft: impl Into<Point<N, Kind>>,
        bottomright: impl Into<Point<N, Kind>>,
    ) -> Self {
        let topleft = topleft.into();
        let bottomright = bottomright.into();
        Rectangle {
            loc: topleft,
            size: (bottomright - topleft).to_size(),
        }
    }

    /// Checks whether given [`Point`] is inside the rectangle
    #[inline]
    pub fn contains<P: Into<Point<N, Kind>>>(self, point: P) -> bool {
        let p: Point<N, Kind> = point.into();
        (p.x >= self.loc.x)
            && (p.x < self.loc.x.saturating_add(self.size.w))
            && (p.y >= self.loc.y)
            && (p.y < self.loc.y.saturating_add(self.size.h))
    }

    /// Checks whether given [`Rectangle`] is inside the rectangle
    #[inline]
    pub fn contains_rect<R: Into<Rectangle<N, Kind>>>(self, rect: R) -> bool {
        let r: Rectangle<N, Kind> = rect.into();
        self.contains(r.loc) && self.contains(r.loc + r.size)
    }

    /// Checks whether a given [`Rectangle`] overlaps with this one
    #[inline]
    pub fn overlaps(self, other: impl Into<Rectangle<N, Kind>>) -> bool {
        let other = other.into();
        // if the rectangle is not outside of the other
        // they must overlap
        !(
            // self is left of other
            self.loc.x.saturating_add(self.size.w) < other.loc.x
            // self is right of other
            ||  self.loc.x > other.loc.x.saturating_add(other.size.w)
            // self is above of other
            ||  self.loc.y.saturating_add(self.size.h) < other.loc.y
            // self is below of other
            ||  self.loc.y > other.loc.y.saturating_add(other.size.h)
        )
    }

    /// Clamp rectangle to min and max corners resulting in the overlapping area of two rectangles
    ///
    /// Returns `None` if the two rectangles don't overlap
    #[inline]
    pub fn intersection(self, other: impl Into<Rectangle<N, Kind>>) -> Option<Self> {
        let other = other.into();
        if !self.overlaps(other) {
            return None;
        }
        Some(Rectangle::from_extemities(
            (self.loc.x.max(other.loc.x), self.loc.y.max(other.loc.y)),
            (
                (self.loc.x.saturating_add(self.size.w)).min(other.loc.x.saturating_add(other.size.w)),
                (self.loc.y.saturating_add(self.size.h)).min(other.loc.y.saturating_add(other.size.h)),
            ),
        ))
    }

    /// Compute the bounding box of a given set of points
    pub fn bounding_box(points: impl IntoIterator<Item = Point<N, Kind>>) -> Self {
        let ret = points.into_iter().fold(None, |acc, point| match acc {
            None => Some((point, point)),
            Some((min_point, max_point)) => Some((
                (point.x.min(min_point.x), point.y.min(min_point.y)).into(),
                (point.x.max(max_point.x), point.y.max(max_point.y)).into(),
            )),
        });

        match ret {
            None => Rectangle::default(),
            Some((min_point, max_point)) => Rectangle::from_extemities(min_point, max_point),
        }
    }

    /// Merge two [`Rectangle`] by producing the smallest rectangle that contains both
    #[inline]
    pub fn merge(self, other: Self) -> Self {
        Self::bounding_box([self.loc, self.loc + self.size, other.loc, other.loc + other.size])
    }
}

impl<N: Coordinate> Rectangle<N, Logical> {
    /// Convert this logical rectangle to physical coordinate space according to given scale factor
    #[inline]
    pub fn to_physical(self, scale: N) -> Rectangle<N, Physical> {
        Rectangle {
            loc: self.loc.to_physical(scale),
            size: self.size.to_physical(scale),
        }
    }

    /// Convert this logical rectangle to buffer coordinate space according to given scale factor
    #[inline]
    pub fn to_buffer(
        self,
        scale: N,
        transformation: Transform,
        area: &Size<N, Logical>,
    ) -> Rectangle<N, Buffer> {
        let rect = transformation.transform_rect_in(self, area);
        Rectangle {
            loc: Point {
                x: rect.loc.x.upscale(scale),
                y: rect.loc.y.upscale(scale),
                _kind: std::marker::PhantomData,
            },
            size: Size {
                w: rect.size.w.upscale(scale),
                h: rect.size.h.upscale(scale),
                _kind: std::marker::PhantomData,
            },
        }
    }
}

impl<N: Coordinate> Rectangle<N, Physical> {
    /// Convert this physical rectangle to logical coordinate space according to given scale factor
    #[inline]
    pub fn to_logical(self, scale: N) -> Rectangle<N, Logical> {
        Rectangle {
            loc: self.loc.to_logical(scale),
            size: self.size.to_logical(scale),
        }
    }
}

impl<N: Coordinate> Rectangle<N, Buffer> {
    /// Convert this physical rectangle to logical coordinate space according to given scale factor
    #[inline]
    pub fn to_logical(
        self,
        scale: N,
        transformation: Transform,
        area: &Size<N, Buffer>,
    ) -> Rectangle<N, Logical> {
        let rect = transformation.invert().transform_rect_in(self, area);
        Rectangle {
            loc: Point {
                x: rect.loc.x.downscale(scale),
                y: rect.loc.y.downscale(scale),
                _kind: std::marker::PhantomData,
            },
            size: Size {
                w: rect.size.w.downscale(scale),
                h: rect.size.h.downscale(scale),
                _kind: std::marker::PhantomData,
            },
        }
    }
}

impl<N: fmt::Debug> fmt::Debug for Rectangle<N, Logical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rectangle<Logical>")
            .field("x", &self.loc.x)
            .field("y", &self.loc.y)
            .field("width", &self.size.w)
            .field("height", &self.size.h)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for Rectangle<N, Physical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rectangle<Physical>")
            .field("x", &self.loc.x)
            .field("y", &self.loc.y)
            .field("width", &self.size.w)
            .field("height", &self.size.h)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for Rectangle<N, Raw> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rectangle<Raw>")
            .field("x", &self.loc.x)
            .field("y", &self.loc.y)
            .field("width", &self.size.w)
            .field("height", &self.size.h)
            .finish()
    }
}

impl<N: fmt::Debug> fmt::Debug for Rectangle<N, Buffer> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rectangle<Buffer>")
            .field("x", &self.loc.x)
            .field("y", &self.loc.y)
            .field("width", &self.size.w)
            .field("height", &self.size.h)
            .finish()
    }
}

impl<N: Clone, Kind> Clone for Rectangle<N, Kind> {
    #[inline]
    fn clone(&self) -> Self {
        Rectangle {
            loc: self.loc.clone(),
            size: self.size.clone(),
        }
    }
}

impl<N: Copy, Kind> Copy for Rectangle<N, Kind> {}

impl<N: PartialEq, Kind> PartialEq for Rectangle<N, Kind> {
    fn eq(&self, other: &Self) -> bool {
        self.loc == other.loc && self.size == other.size
    }
}

impl<N: Eq, Kind> Eq for Rectangle<N, Kind> {}

impl<N: Default, Kind> Default for Rectangle<N, Kind> {
    fn default() -> Self {
        Rectangle {
            loc: Default::default(),
            size: Default::default(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
/// Possible transformations to two-dimensional planes
pub enum Transform {
    /// Identity transformation (plane is unaltered when applied)
    Normal,
    /// Plane is rotated by 90 degrees
    _90,
    /// Plane is rotated by 180 degrees
    _180,
    /// Plane is rotated by 270 degrees
    _270,
    /// Plane is flipped vertically
    Flipped,
    /// Plane is flipped vertically and rotated by 90 degrees
    Flipped90,
    /// Plane is flipped vertically and rotated by 180 degrees
    Flipped180,
    /// Plane is flipped vertically and rotated by 270 degrees
    Flipped270,
}

impl Default for Transform {
    fn default() -> Transform {
        Transform::Normal
    }
}

impl Transform {
    /// Inverts any 90-degree transformation into 270-degree transformations and vise versa.
    ///
    /// Flipping is preserved and 180/Normal transformation are uneffected.
    pub fn invert(&self) -> Transform {
        match self {
            Transform::Normal => Transform::Normal,
            Transform::Flipped => Transform::Flipped,
            Transform::_90 => Transform::_270,
            Transform::_180 => Transform::_180,
            Transform::_270 => Transform::_90,
            Transform::Flipped90 => Transform::Flipped270,
            Transform::Flipped180 => Transform::Flipped180,
            Transform::Flipped270 => Transform::Flipped90,
        }
    }

    /// Transforms a point inside an area of a given size by applying this transformation.
    pub fn transform_point_in<N: Coordinate, Kind>(
        &self,
        point: Point<N, Kind>,
        area: &Size<N, Kind>,
    ) -> Point<N, Kind> {
        match *self {
            Transform::Normal => point,
            Transform::_90 => (area.h - point.y, point.x).into(),
            Transform::_180 => (area.w - point.x, area.h - point.y).into(),
            Transform::_270 => (point.y, area.w - point.x).into(),
            Transform::Flipped => (area.w - point.x, point.y).into(),
            Transform::Flipped90 => (point.y, point.x).into(),
            Transform::Flipped180 => (point.x, area.h - point.y).into(),
            Transform::Flipped270 => (area.h - point.y, area.w - point.x).into(),
        }
    }

    /// Transformed size after applying this transformation.
    pub fn transform_size<N: Coordinate, Kind>(&self, size: Size<N, Kind>) -> Size<N, Kind> {
        if *self == Transform::_90
            || *self == Transform::_270
            || *self == Transform::Flipped90
            || *self == Transform::Flipped270
        {
            (size.h, size.w).into()
        } else {
            size
        }
    }

    /// Transforms a rectangle inside an area of a given size by applying this transformation.
    pub fn transform_rect_in<N: Coordinate, Kind>(
        &self,
        rect: Rectangle<N, Kind>,
        area: &Size<N, Kind>,
    ) -> Rectangle<N, Kind> {
        let size = self.transform_size(rect.size);

        let loc = match *self {
            Transform::Normal => rect.loc,
            Transform::_90 => (area.h - rect.loc.y - rect.size.h, rect.loc.x).into(),
            Transform::_180 => (
                area.w - rect.loc.x - rect.size.w,
                area.h - rect.loc.y - rect.size.h,
            )
                .into(),
            Transform::_270 => (rect.loc.y, area.w - rect.loc.x - rect.size.w).into(),
            Transform::Flipped => (area.w - rect.loc.x - rect.size.w, rect.loc.y).into(),
            Transform::Flipped90 => (rect.loc.y, rect.loc.x).into(),
            Transform::Flipped180 => (rect.loc.x, area.h - rect.loc.y - rect.size.h).into(),
            Transform::Flipped270 => (
                area.h - rect.loc.y - rect.size.h,
                area.w - rect.loc.x - rect.size.w,
            )
                .into(),
        };

        Rectangle::from_loc_and_size(loc, size)
    }

    /// Returns true if the transformation would flip contents
    pub fn flipped(&self) -> bool {
        !matches!(
            self,
            Transform::Normal | Transform::_90 | Transform::_180 | Transform::_270
        )
    }

    /// Returns the angle (in degrees) of the transformation
    pub fn degrees(&self) -> u32 {
        match self {
            Transform::Normal | Transform::Flipped => 0,
            Transform::_90 | Transform::Flipped90 => 90,
            Transform::_180 | Transform::Flipped180 => 180,
            Transform::_270 | Transform::Flipped270 => 270,
        }
    }
}

impl std::ops::Add for Transform {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        let flipped = matches!((self.flipped(), other.flipped()), (true, false) | (false, true));
        let degrees = (self.degrees() + other.degrees()) % 360;
        match (flipped, degrees) {
            (false, 0) => Transform::Normal,
            (false, 90) => Transform::_90,
            (false, 180) => Transform::_180,
            (false, 270) => Transform::_270,
            (true, 0) => Transform::Flipped,
            (true, 90) => Transform::Flipped90,
            (true, 180) => Transform::Flipped180,
            (true, 270) => Transform::Flipped270,
            _ => unreachable!(),
        }
    }
}

#[cfg(feature = "wayland_frontend")]
impl From<Transform> for WlTransform {
    fn from(transform: Transform) -> Self {
        match transform {
            Transform::Normal => WlTransform::Normal,
            Transform::_90 => WlTransform::_90,
            Transform::_180 => WlTransform::_180,
            Transform::_270 => WlTransform::_270,
            Transform::Flipped => WlTransform::Flipped,
            Transform::Flipped90 => WlTransform::Flipped90,
            Transform::Flipped180 => WlTransform::Flipped180,
            Transform::Flipped270 => WlTransform::Flipped270,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Logical, Rectangle, Size, Transform};

    #[test]
    fn transform_rect_ident() {
        let rect = Rectangle::<i32, Logical>::from_loc_and_size((10, 20), (30, 40));
        let size = Size::from((70, 90));
        let transform = Transform::Normal;

        assert_eq!(rect, transform.transform_rect_in(rect, &size))
    }

    #[test]
    fn transform_rect_90() {
        let rect = Rectangle::<i32, Logical>::from_loc_and_size((10, 20), (30, 40));
        let size = Size::from((70, 90));
        let transform = Transform::_90;

        assert_eq!(
            Rectangle::from_loc_and_size((30, 10), (40, 30)),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn transform_rect_180() {
        let rect = Rectangle::<i32, Logical>::from_loc_and_size((10, 20), (30, 40));
        let size = Size::from((70, 90));
        let transform = Transform::_180;

        assert_eq!(
            Rectangle::from_loc_and_size((30, 30), (30, 40)),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn transform_rect_270() {
        let rect = Rectangle::<i32, Logical>::from_loc_and_size((10, 20), (30, 40));
        let size = Size::from((70, 90));
        let transform = Transform::_270;

        assert_eq!(
            Rectangle::from_loc_and_size((20, 30), (40, 30)),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn transform_rect_f() {
        let rect = Rectangle::<i32, Logical>::from_loc_and_size((10, 20), (30, 40));
        let size = Size::from((70, 90));
        let transform = Transform::Flipped;

        assert_eq!(
            Rectangle::from_loc_and_size((30, 20), (30, 40)),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn transform_rect_f90() {
        let rect = Rectangle::<i32, Logical>::from_loc_and_size((10, 20), (30, 40));
        let size = Size::from((70, 80));
        let transform = Transform::Flipped90;

        assert_eq!(
            Rectangle::from_loc_and_size((20, 10), (40, 30)),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn transform_rect_f180() {
        let rect = Rectangle::<i32, Logical>::from_loc_and_size((10, 20), (30, 40));
        let size = Size::from((70, 90));
        let transform = Transform::Flipped180;

        assert_eq!(
            Rectangle::from_loc_and_size((10, 30), (30, 40)),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn transform_rect_f270() {
        let rect = Rectangle::<i32, Logical>::from_loc_and_size((10, 20), (30, 40));
        let size = Size::from((70, 90));
        let transform = Transform::Flipped270;

        assert_eq!(
            Rectangle::from_loc_and_size((30, 30), (40, 30)),
            transform.transform_rect_in(rect, &size)
        )
    }
}
