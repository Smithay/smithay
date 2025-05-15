use std::fmt;
use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};

#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::wl_output::Transform as WlTransform;

/// Type-level marker for the logical coordinate space
#[derive(Debug)]
pub struct Logical;

/// Type-level marker for the client logical coordinate space
#[derive(Debug)]
#[cfg(feature = "wayland_frontend")]
pub(crate) struct Client;

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
    /// A Coordinate that is 0
    const ZERO: Self;
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
            const ZERO: $ty = 0;

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
            const ZERO: $ty = 0;

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
            const ZERO: $ty = 0.0;

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
 * Scale
 */

/// A two-dimensional scale that can be
/// used to scale [`Point`]s, [`Size`]s and
/// [`Rectangle`]s
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale<N: Coordinate> {
    /// The scale on the x axis
    pub x: N,
    /// The scale on the y axis
    pub y: N,
}

impl<N: Coordinate> Scale<N> {
    /// Convert the underlying numerical type to f64 for floating point manipulations
    #[inline]
    pub fn to_f64(self) -> Scale<f64> {
        Scale {
            x: self.x.to_f64(),
            y: self.y.to_f64(),
        }
    }
}

impl<N: Coordinate> From<N> for Scale<N> {
    #[inline]
    fn from(scale: N) -> Self {
        Scale { x: scale, y: scale }
    }
}

impl<N: Coordinate> From<(N, N)> for Scale<N> {
    #[inline]
    fn from((scale_x, scale_y): (N, N)) -> Self {
        Scale {
            x: scale_x,
            y: scale_y,
        }
    }
}

impl<N, T> Mul<T> for Scale<N>
where
    N: Coordinate,
    T: Into<Scale<N>>,
{
    type Output = Scale<N>;

    #[inline]
    fn mul(self, rhs: T) -> Self::Output {
        let rhs = rhs.into();
        Scale {
            x: self.x.upscale(rhs.x),
            y: self.y.upscale(rhs.y),
        }
    }
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

    /// Upscale this [`Point`] by a specified [`Scale`]
    #[inline]
    pub fn upscale(self, scale: impl Into<Scale<N>>) -> Point<N, Kind> {
        let scale = scale.into();
        Point {
            x: self.x.upscale(scale.x),
            y: self.y.upscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }

    /// Downscale this [`Point`] by a specified [`Scale`]
    #[inline]
    pub fn downscale(self, scale: impl Into<Scale<N>>) -> Point<N, Kind> {
        let scale = scale.into();
        Point {
            x: self.x.downscale(scale.x),
            y: self.y.downscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate, Kind> Point<N, Kind> {
    /// Constrain this [`Point`] within a [`Rectangle`] with the same coordinates
    ///
    /// The [`Point`] returned is guaranteed to be not smaller than the [`Rectangle`]
    /// location and not greater than the [`Rectangle`] location plus size.
    #[inline]
    pub fn constrain(self, rect: impl Into<Rectangle<N, Kind>>) -> Point<N, Kind> {
        let rect = rect.into();

        Point {
            x: self.x.max(rect.loc.x).min(rect.loc.x + rect.size.w),
            y: self.y.max(rect.loc.y).min(rect.loc.y + rect.size.h),
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

impl<N: fmt::Debug, S> fmt::Debug for Point<N, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("Point<{}>", std::any::type_name::<S>()))?;
        f.debug_struct("")
            .field("x", &self.x)
            .field("y", &self.y)
            .finish()
    }
}

impl<N: Coordinate> Point<N, Logical> {
    #[inline]
    #[cfg(feature = "wayland_frontend")]
    pub(crate) fn to_client(self, scale: impl Into<Scale<N>>) -> Point<N, Client> {
        let scale: Scale<N> = scale.into();
        Point {
            x: self.x.upscale(scale.x),
            y: self.y.upscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }

    #[inline]
    #[cfg(feature = "wayland_frontend")]
    pub(crate) fn to_client_precise_round<S: Coordinate, R: Coordinate>(
        self,
        scale: impl Into<Scale<S>>,
    ) -> Point<R, Client> {
        self.to_f64().to_client(scale.into().to_f64()).to_i32_round()
    }

    #[inline]
    /// Convert this logical point to physical coordinate space according to given scale factor
    pub fn to_physical(self, scale: impl Into<Scale<N>>) -> Point<N, Physical> {
        let scale = scale.into();
        Point {
            x: self.x.upscale(scale.x),
            y: self.y.upscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }

    /// Convert this logical point to physical coordinate space according to given scale factor
    /// and round the result
    #[inline]
    pub fn to_physical_precise_round<S: Coordinate, R: Coordinate>(
        self,
        scale: impl Into<Scale<S>>,
    ) -> Point<R, Physical> {
        self.to_f64().to_physical(scale.into().to_f64()).to_i32_round()
    }

    /// Convert this logical point to physical coordinate space according to given scale factor
    /// and ceil the result
    #[inline]
    pub fn to_physical_precise_ceil<S: Coordinate, R: Coordinate>(
        &self,
        scale: impl Into<Scale<S>>,
    ) -> Point<R, Physical> {
        self.to_f64().to_physical(scale.into().to_f64()).to_i32_ceil()
    }

    /// Convert this logical point to physical coordinate space according to given scale factor
    /// and floor the result
    #[inline]
    pub fn to_physical_precise_floor<S: Coordinate, R: Coordinate>(
        &self,
        scale: impl Into<Scale<S>>,
    ) -> Point<R, Physical> {
        self.to_f64().to_physical(scale.into().to_f64()).to_i32_floor()
    }

    #[inline]
    /// Convert this logical point to buffer coordinate space according to given scale factor
    pub fn to_buffer(
        self,
        scale: impl Into<Scale<N>>,
        transformation: Transform,
        area: &Size<N, Logical>,
    ) -> Point<N, Buffer> {
        let point = transformation.transform_point_in(self, area);
        let scale = scale.into();
        Point {
            x: point.x.upscale(scale.x),
            y: point.y.upscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }
}

#[cfg(feature = "wayland_frontend")]
impl<N: Coordinate> Point<N, Client> {
    #[inline]
    pub(crate) fn to_logical(self, scale: impl Into<Scale<N>>) -> Point<N, Logical> {
        let scale = scale.into();
        Point {
            x: self.x.downscale(scale.x),
            y: self.y.downscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate> Point<N, Physical> {
    #[inline]
    /// Convert this physical point to logical coordinate space according to given scale factor
    pub fn to_logical(self, scale: impl Into<Scale<N>>) -> Point<N, Logical> {
        let scale = scale.into();
        Point {
            x: self.x.downscale(scale.x),
            y: self.y.downscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate> Point<N, Buffer> {
    #[inline]
    /// Convert this physical point to logical coordinate space according to given scale factor
    pub fn to_logical(
        self,
        scale: impl Into<Scale<N>>,
        transform: Transform,
        area: &Size<N, Buffer>,
    ) -> Point<N, Logical> {
        let point = transform.invert().transform_point_in(self, area);
        let scale = scale.into();
        Point {
            x: point.x.downscale(scale.x),
            y: point.y.downscale(scale.y),
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
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.x == other.x && self.y == other.y
    }
}

impl<N: Eq, Kind> Eq for Point<N, Kind> {}

impl<N: Default, Kind> Default for Point<N, Kind> {
    #[inline]
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

impl<N: Coordinate, Kind> Size<N, Kind> {
    /// Upscale this [`Size`] by a specified [`Scale`]
    #[inline]
    pub fn upscale(self, scale: impl Into<Scale<N>>) -> Size<N, Kind> {
        let scale = scale.into();
        Size {
            w: self.w.upscale(scale.x),
            h: self.h.upscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }

    /// Downscale this [`Size`] by a specified [`Scale`]
    #[inline]
    pub fn downscale(self, scale: impl Into<Scale<N>>) -> Size<N, Kind> {
        let scale = scale.into();
        Size {
            w: self.w.downscale(scale.x),
            h: self.h.downscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }

    /// Check if this [`Size`] is empty
    ///
    /// Returns true if either the width or the height is zero
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.w == N::default() || self.h == N::default()
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

impl<N: fmt::Debug, S> fmt::Debug for Size<N, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("Size<{}>", std::any::type_name::<S>()))?;
        f.debug_struct("")
            .field("w", &self.w)
            .field("h", &self.h)
            .finish()
    }
}

impl<N: Coordinate> Size<N, Logical> {
    #[inline]
    #[cfg(feature = "wayland_frontend")]
    pub(crate) fn to_client(self, scale: impl Into<Scale<N>>) -> Size<N, Client> {
        let scale = scale.into();
        Size {
            w: self.w.upscale(scale.x),
            h: self.h.upscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }

    #[inline]
    /// Convert this logical size to physical coordinate space according to given scale factor
    pub fn to_physical(self, scale: impl Into<Scale<N>>) -> Size<N, Physical> {
        let scale = scale.into();
        Size {
            w: self.w.upscale(scale.x),
            h: self.h.upscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }

    /// Convert this logical size to physical coordinate space according to given scale factor
    /// and round the result
    #[inline]
    pub fn to_physical_precise_round<S: Coordinate, R: Coordinate>(
        self,
        scale: impl Into<Scale<S>>,
    ) -> Size<R, Physical> {
        self.to_f64().to_physical(scale.into().to_f64()).to_i32_round()
    }

    /// Convert this logical size to physical coordinate space according to given scale factor
    /// and ceil the result
    #[inline]
    pub fn to_physical_precise_ceil<S: Coordinate, R: Coordinate>(
        &self,
        scale: impl Into<Scale<S>>,
    ) -> Size<R, Physical> {
        self.to_f64().to_physical(scale.into().to_f64()).to_i32_ceil()
    }

    /// Convert this logical size to physical coordinate space according to given scale factor
    /// and floor the result
    #[inline]
    pub fn to_physical_precise_floor<S: Coordinate, R: Coordinate>(
        &self,
        scale: impl Into<Scale<S>>,
    ) -> Size<R, Physical> {
        self.to_f64().to_physical(scale.into().to_f64()).to_i32_floor()
    }

    #[inline]
    /// Convert this logical size to buffer coordinate space according to given scale factor
    pub fn to_buffer(self, scale: impl Into<Scale<N>>, transformation: Transform) -> Size<N, Buffer> {
        let scale = scale.into();
        transformation.transform_size(Size {
            w: self.w.upscale(scale.x),
            h: self.h.upscale(scale.y),
            _kind: std::marker::PhantomData,
        })
    }
}

#[cfg(feature = "wayland_frontend")]
impl<N: Coordinate> Size<N, Client> {
    #[inline]
    pub(crate) fn to_logical(self, scale: impl Into<Scale<N>>) -> Size<N, Logical> {
        let scale = scale.into();
        Size {
            w: self.w.downscale(scale.x),
            h: self.h.downscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate> Size<N, Physical> {
    #[inline]
    /// Convert this physical point to logical coordinate space according to given scale factor
    pub fn to_logical(self, scale: impl Into<Scale<N>>) -> Size<N, Logical> {
        let scale = scale.into();
        Size {
            w: self.w.downscale(scale.x),
            h: self.h.downscale(scale.y),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate> Size<N, Buffer> {
    #[inline]
    /// Convert this physical point to logical coordinate space according to given scale factor
    pub fn to_logical(self, scale: impl Into<Scale<N>>, transformation: Transform) -> Size<N, Logical> {
        let scale = scale.into();
        transformation.invert().transform_size(Size {
            w: self.w.downscale(scale.x),
            h: self.h.downscale(scale.y),
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

impl<N: Coordinate, Kind> Sub for Size<N, Kind> {
    type Output = Size<N, Kind>;
    #[inline]
    fn sub(self, rhs: Self) -> Self::Output {
        Size {
            w: self.w.saturating_sub(rhs.w).max(N::ZERO),
            h: self.h.saturating_sub(rhs.h).max(N::ZERO),
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate, Kind> SubAssign for Size<N, Kind> {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.w = self.w.saturating_sub(rhs.w).max(N::ZERO);
        self.h = self.h.saturating_sub(rhs.h).max(N::ZERO);
    }
}

impl<N: Coordinate + Div<Output = N>, KindLhs, KindRhs> Div<Size<N, KindRhs>> for Size<N, KindLhs> {
    type Output = Scale<N>;

    #[inline]
    fn div(self, rhs: Size<N, KindRhs>) -> Self::Output {
        Scale {
            x: self.w / rhs.w,
            y: self.h / rhs.h,
        }
    }
}

impl<N: Coordinate + Div, Kind> Div<N> for Size<N, Kind> {
    type Output = Size<<N as Div>::Output, Kind>;

    #[inline]
    fn div(self, rhs: N) -> Self::Output {
        Size {
            w: self.w / rhs,
            h: self.h / rhs,
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Coordinate + Mul, Kind> Mul<N> for Size<N, Kind> {
    type Output = Size<<N as Mul>::Output, Kind>;

    #[inline]
    fn mul(self, rhs: N) -> Self::Output {
        Size {
            w: self.w * rhs,
            h: self.h * rhs,
            _kind: std::marker::PhantomData,
        }
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
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.w == other.w && self.h == other.h
    }
}

impl<N: Eq, Kind> Eq for Size<N, Kind> {}

impl<N: Default, Kind> Default for Size<N, Kind> {
    #[inline]
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
/// Operations on rectangles are saturating.
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

impl<N: Coordinate, Kind> Rectangle<N, Kind> {
    /// Upscale this [`Rectangle`] by the supplied [`Scale`]
    pub fn upscale(self, scale: impl Into<Scale<N>>) -> Rectangle<N, Kind> {
        let scale = scale.into();
        Rectangle {
            loc: self.loc.upscale(scale),
            size: self.size.upscale(scale),
        }
    }

    /// Downscale this [`Rectangle`] by the supplied [`Scale`]
    pub fn downscale(self, scale: impl Into<Scale<N>>) -> Rectangle<N, Kind> {
        let scale = scale.into();
        Rectangle {
            loc: self.loc.downscale(scale),
            size: self.size.downscale(scale),
        }
    }

    /// Check if this [`Rectangle`] is empty
    ///
    /// Returns true if either the width or the height
    /// of the [`Size`] is zero
    pub fn is_empty(&self) -> bool {
        self.size.is_empty()
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
        Rectangle::from_extremities(self.loc.to_i32_ceil(), (self.loc + self.size).to_i32_floor())
    }

    /// Convert to i32 by returning the smallest integet-space rectangle encapsulating the float-based rectangle
    #[inline]
    pub fn to_i32_up<N: Coordinate>(self) -> Rectangle<N, Kind> {
        Rectangle::from_extremities(self.loc.to_i32_floor(), (self.loc + self.size).to_i32_ceil())
    }
}

impl<N: Coordinate, Kind> Rectangle<N, Kind> {
    /// Create a new [`Rectangle`] from the coordinates of its top-left corner and its dimensions
    #[deprecated(note = "use new or from_size")]
    #[inline]
    pub fn from_loc_and_size(loc: impl Into<Point<N, Kind>>, size: impl Into<Size<N, Kind>>) -> Self {
        Rectangle {
            loc: loc.into(),
            size: size.into(),
        }
    }

    /// Create a new [`Rectangle`] from the coordinates of its top-left corner and its dimensions
    #[inline]
    pub fn new(loc: Point<N, Kind>, size: Size<N, Kind>) -> Self {
        Rectangle { loc, size }
    }

    /// Create a new [`Rectangle`] from its dimensions, with location zero
    #[inline]
    pub fn from_size(size: Size<N, Kind>) -> Self {
        Rectangle {
            loc: (N::ZERO, N::ZERO).into(),
            size,
        }
    }

    /// Create a new [`Rectangle`] with location and size zero
    #[inline]
    pub fn zero() -> Self {
        Rectangle {
            loc: (N::ZERO, N::ZERO).into(),
            size: (N::ZERO, N::ZERO).into(),
        }
    }

    /// Create a new [`Rectangle`] from the coordinates of its top-left corner and its bottom-right corner
    #[inline]
    #[deprecated = "use Rectangle::from_extremities instead"]
    #[doc(hidden)]
    pub fn from_extemities(
        topleft: impl Into<Point<N, Kind>>,
        bottomright: impl Into<Point<N, Kind>>,
    ) -> Self {
        Rectangle::from_extremities(topleft, bottomright)
    }

    /// Create a new [`Rectangle`] from the coordinates of its top-left corner and its bottom-right corner
    #[inline]
    pub fn from_extremities(
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
    ///
    /// A rectangle is considered inside another rectangle
    /// if its location is inside the other rectangle and it does not
    /// extend outside the other rectangle.
    /// This includes rectangles with the same location and size
    #[inline]
    pub fn contains_rect<R: Into<Rectangle<N, Kind>>>(self, rect: R) -> bool {
        let r: Rectangle<N, Kind> = rect.into();
        r.loc.x >= self.loc.x
            && r.loc.y >= self.loc.y
            && r.loc.x.saturating_add(r.size.w) <= self.loc.x.saturating_add(self.size.w)
            && r.loc.y.saturating_add(r.size.h) <= self.loc.y.saturating_add(self.size.h)
    }

    /// Checks whether a given [`Rectangle`] overlaps with this one
    ///
    /// Note: This operation is exclusive, touching only rectangles will return `false`.
    /// For inclusive overlap test see [`overlaps_or_touches`](Rectangle::overlaps_or_touches)
    #[inline]
    pub fn overlaps(self, other: impl Into<Rectangle<N, Kind>>) -> bool {
        let other = other.into();

        self.loc.x < other.loc.x.saturating_add(other.size.w)
            && other.loc.x < self.loc.x.saturating_add(self.size.w)
            && self.loc.y < other.loc.y.saturating_add(other.size.h)
            && other.loc.y < self.loc.y.saturating_add(self.size.h)
    }

    /// Checks whether a given [`Rectangle`] overlaps with this one or touches it
    ///
    /// Note: This operation is inclusive, touching only rectangles will return `true`.
    /// For exclusive overlap test see [`overlaps`](Rectangle::overlaps)
    #[inline]
    pub fn overlaps_or_touches(self, other: impl Into<Rectangle<N, Kind>>) -> bool {
        let other = other.into();

        self.loc.x <= other.loc.x.saturating_add(other.size.w)
            && other.loc.x <= self.loc.x.saturating_add(self.size.w)
            && self.loc.y <= other.loc.y.saturating_add(other.size.h)
            && other.loc.y <= self.loc.y.saturating_add(self.size.h)
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
        Some(Rectangle::from_extremities(
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
            Some((min_point, max_point)) => Rectangle::from_extremities(min_point, max_point),
        }
    }

    /// Merge two [`Rectangle`] by producing the smallest rectangle that contains both
    #[inline]
    pub fn merge(self, other: Self) -> Self {
        Self::bounding_box([self.loc, self.loc + self.size, other.loc, other.loc + other.size])
    }

    /// Subtract another [`Rectangle`] from this [`Rectangle`]
    ///
    /// If the rectangles to not overlap the original rectangle will
    /// be returned.
    /// If the other rectangle contains self no rectangle will be returned,
    /// otherwise up to 4 rectangles will be returned.
    pub fn subtract_rect(self, other: Self) -> Vec<Self> {
        self.subtract_rects([other])
    }

    /// Subtract a set of [`Rectangle`]s from this [`Rectangle`]
    pub fn subtract_rects(self, others: impl IntoIterator<Item = Self>) -> Vec<Self> {
        let mut remaining = Vec::with_capacity(4);
        remaining.push(self);
        Self::subtract_rects_many_in_place(remaining, others)
    }

    /// Subtract a set of [`Rectangle`]s from a set [`Rectangle`]s
    pub fn subtract_rects_many(
        rects: impl IntoIterator<Item = Self>,
        others: impl IntoIterator<Item = Self>,
    ) -> Vec<Self> {
        let remaining = rects.into_iter().collect::<Vec<_>>();
        Self::subtract_rects_many_in_place(remaining, others)
    }

    /// Subtract a set of [`Rectangle`]s from a set [`Rectangle`]s in-place
    pub fn subtract_rects_many_in_place(
        mut rects: Vec<Self>,
        others: impl IntoIterator<Item = Self>,
    ) -> Vec<Self> {
        for other in others {
            let items = rects.len();
            let mut checked = 0usize;
            let mut index = 0usize;

            // If there is nothing left we can stop,
            // we won't be able to subtract any further
            if items == 0 {
                return rects;
            }

            while checked != items {
                checked += 1;

                // If there is no overlap there is nothing to subtract
                let Some(intersection) = rects[index].intersection(other) else {
                    index += 1;
                    continue;
                };

                // We now know that we have to subtract the other rect
                let item = rects.remove(index);

                // If we are completely contained then nothing is left
                if other.contains_rect(item) {
                    continue;
                }

                let top_rect = Rectangle::new(
                    item.loc,
                    (item.size.w, intersection.loc.y.saturating_sub(item.loc.y)).into(),
                );
                let left_rect: Rectangle<N, Kind> = Rectangle::new(
                    (item.loc.x, intersection.loc.y).into(),
                    (intersection.loc.x.saturating_sub(item.loc.x), intersection.size.h).into(),
                );
                let right_rect: Rectangle<N, Kind> = Rectangle::new(
                    (
                        intersection.loc.x.saturating_add(intersection.size.w),
                        intersection.loc.y,
                    )
                        .into(),
                    (
                        (item.loc.x.saturating_add(item.size.w))
                            .saturating_sub(intersection.loc.x.saturating_add(intersection.size.w)),
                        intersection.size.h,
                    )
                        .into(),
                );
                let bottom_rect: Rectangle<N, Kind> = Rectangle::new(
                    (item.loc.x, intersection.loc.y.saturating_add(intersection.size.h)).into(),
                    (
                        item.size.w,
                        (item.loc.y.saturating_add(item.size.h))
                            .saturating_sub(intersection.loc.y.saturating_add(intersection.size.h)),
                    )
                        .into(),
                );

                if !top_rect.is_empty() {
                    rects.push(top_rect);
                }

                if !left_rect.is_empty() {
                    rects.push(left_rect);
                }

                if !right_rect.is_empty() {
                    rects.push(right_rect);
                }

                if !bottom_rect.is_empty() {
                    rects.push(bottom_rect);
                }
            }
        }

        rects
    }
}

impl<N: Coordinate> Rectangle<N, Logical> {
    #[inline]
    #[cfg(feature = "xwayland")]
    pub(crate) fn to_client(self, scale: impl Into<Scale<N>>) -> Rectangle<N, Client> {
        let scale = scale.into();
        Rectangle {
            loc: self.loc.to_client(scale),
            size: self.size.to_client(scale),
        }
    }

    #[inline]
    #[cfg(feature = "xwayland")]
    pub(crate) fn to_client_precise_round<S: Coordinate, R: Coordinate>(
        self,
        scale: impl Into<Scale<S>>,
    ) -> Rectangle<R, Client> {
        self.to_f64().to_client(scale.into().to_f64()).to_i32_round()
    }

    /// Convert this logical rectangle to physical coordinate space according to given scale factor
    #[inline]
    pub fn to_physical(self, scale: impl Into<Scale<N>>) -> Rectangle<N, Physical> {
        let scale = scale.into();
        Rectangle {
            loc: self.loc.to_physical(scale),
            size: self.size.to_physical(scale),
        }
    }

    /// Convert this logical rectangle to physical coordinate space according to given scale factor
    /// and round the result
    #[inline]
    pub fn to_physical_precise_round<S: Coordinate, R: Coordinate>(
        self,
        scale: impl Into<Scale<S>>,
    ) -> Rectangle<R, Physical> {
        self.to_f64().to_physical(scale.into().to_f64()).to_i32_round()
    }

    /// Convert this logical rectangle to physical coordinate space according to given scale factor,
    /// returning the largest N-space rectangle fitting into the N-based rectangle
    ///
    /// This will ceil the location and floor the size after applying the scale
    #[inline]
    pub fn to_physical_precise_down<S: Coordinate, R: Coordinate>(
        &self,
        scale: impl Into<Scale<S>>,
    ) -> Rectangle<R, Physical> {
        self.to_f64().to_physical(scale.into().to_f64()).to_i32_down()
    }

    /// Convert this logical rectangle to physical coordinate space according to given scale factor,
    /// returning the smallest N-space rectangle encapsulating the N-based rectangle
    ///
    /// This will floor the location and ceil the size after applying the scale
    #[inline]
    pub fn to_physical_precise_up<S: Coordinate, R: Coordinate>(
        &self,
        scale: impl Into<Scale<S>>,
    ) -> Rectangle<R, Physical> {
        self.to_f64().to_physical(scale.into().to_f64()).to_i32_up()
    }

    /// Convert this logical rectangle to buffer coordinate space according to given scale factor
    #[inline]
    pub fn to_buffer(
        self,
        scale: impl Into<Scale<N>>,
        transformation: Transform,
        area: &Size<N, Logical>,
    ) -> Rectangle<N, Buffer> {
        let rect = transformation.transform_rect_in(self, area);
        let scale = scale.into();
        Rectangle {
            loc: Point {
                x: rect.loc.x.upscale(scale.x),
                y: rect.loc.y.upscale(scale.y),
                _kind: std::marker::PhantomData,
            },
            size: Size {
                w: rect.size.w.upscale(scale.x),
                h: rect.size.h.upscale(scale.y),
                _kind: std::marker::PhantomData,
            },
        }
    }
}

#[cfg(feature = "wayland_frontend")]
impl<N: Coordinate> Rectangle<N, Client> {
    #[inline]
    pub(crate) fn to_logical(self, scale: impl Into<Scale<N>>) -> Rectangle<N, Logical> {
        let scale = scale.into();
        Rectangle {
            loc: self.loc.to_logical(scale),
            size: self.size.to_logical(scale),
        }
    }
}

impl<N: Coordinate> Rectangle<N, Physical> {
    /// Convert this physical rectangle to logical coordinate space according to given scale factor
    #[inline]
    pub fn to_logical(self, scale: impl Into<Scale<N>>) -> Rectangle<N, Logical> {
        let scale = scale.into();
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
        scale: impl Into<Scale<N>>,
        transformation: Transform,
        area: &Size<N, Buffer>,
    ) -> Rectangle<N, Logical> {
        let rect = transformation.invert().transform_rect_in(self, area);
        let scale = scale.into();
        Rectangle {
            loc: Point {
                x: rect.loc.x.downscale(scale.x),
                y: rect.loc.y.downscale(scale.y),
                _kind: std::marker::PhantomData,
            },
            size: Size {
                w: rect.size.w.downscale(scale.x),
                h: rect.size.h.downscale(scale.y),
                _kind: std::marker::PhantomData,
            },
        }
    }
}

impl<N: fmt::Debug, S> fmt::Debug for Rectangle<N, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("Rectangle<{}>", std::any::type_name::<S>()))?;
        f.debug_struct("")
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
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.loc == other.loc && self.size == other.size
    }
}

impl<N: Eq, Kind> Eq for Rectangle<N, Kind> {}

impl<N: Default, Kind> Default for Rectangle<N, Kind> {
    #[inline]
    fn default() -> Self {
        Rectangle {
            loc: Default::default(),
            size: Default::default(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
/// Possible transformations to two-dimensional planes
#[derive(Default)]
pub enum Transform {
    /// Identity transformation (plane is unaltered when applied)
    #[default]
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

impl Transform {
    /// Inverts any 90-degree transformation into 270-degree transformations and vise versa.
    ///
    /// Flipping is preserved and 180/Normal transformation are uneffected.
    #[inline]
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
            Transform::Flipped90 => (
                area.h - rect.loc.y - rect.size.h,
                area.w - rect.loc.x - rect.size.w,
            )
                .into(),
            Transform::Flipped180 => (rect.loc.x, area.h - rect.loc.y - rect.size.h).into(),
            Transform::Flipped270 => (rect.loc.y, rect.loc.x).into(),
        };

        Rectangle::new(loc, size)
    }

    /// Returns true if the transformation would flip contents
    pub fn flipped(&self) -> bool {
        !matches!(
            self,
            Transform::Normal | Transform::_90 | Transform::_180 | Transform::_270
        )
    }

    /// Returns the angle (in degrees) of the transformation
    #[inline]
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

    #[inline]
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
    #[inline]
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
        let rect = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        let size = Size::from((70, 90));
        let transform = Transform::Normal;

        assert_eq!(rect, transform.transform_rect_in(rect, &size))
    }

    #[test]
    fn transform_rect_90() {
        let rect = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        let size = Size::from((70, 90));
        let transform = Transform::_90;

        assert_eq!(
            Rectangle::new((30, 10).into(), (40, 30).into()),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn transform_rect_180() {
        let rect = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        let size = Size::from((70, 90));
        let transform = Transform::_180;

        assert_eq!(
            Rectangle::new((30, 30).into(), (30, 40).into()),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn transform_rect_270() {
        let rect = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        let size = Size::from((70, 90));
        let transform = Transform::_270;

        assert_eq!(
            Rectangle::new((20, 30).into(), (40, 30).into()),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn transform_rect_f() {
        let rect = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        let size = Size::from((70, 90));
        let transform = Transform::Flipped;

        assert_eq!(
            Rectangle::new((30, 20).into(), (30, 40).into()),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn transform_rect_f90() {
        let rect = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        let size = Size::from((70, 80));
        let transform = Transform::Flipped90;

        assert_eq!(
            Rectangle::new((20, 30).into(), (40, 30).into()),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn transform_rect_f180() {
        let rect = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        let size = Size::from((70, 90));
        let transform = Transform::Flipped180;

        assert_eq!(
            Rectangle::new((10, 30).into(), (30, 40).into()),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn transform_rect_f270() {
        let rect = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        let size = Size::from((70, 90));
        let transform = Transform::Flipped270;

        assert_eq!(
            Rectangle::new((20, 10).into(), (40, 30).into()),
            transform.transform_rect_in(rect, &size)
        )
    }

    #[test]
    fn rectangle_contains_rect_itself() {
        let rect = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        assert!(rect.contains_rect(rect));
    }

    #[test]
    fn rectangle_contains_rect_outside() {
        let first = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        let second = Rectangle::<i32, Logical>::new((41, 61).into(), (30, 40).into());
        assert!(!first.contains_rect(second));
    }

    #[test]
    fn rectangle_contains_rect_extends() {
        let first = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        let second = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 45).into());
        assert!(!first.contains_rect(second));
    }

    #[test]
    fn rectangle_subtract_full() {
        let outer = Rectangle::<i32, Logical>::from_size((100, 100).into());
        let inner = Rectangle::<i32, Logical>::new((-10, -10).into(), (1000, 1000).into());

        let rects = outer.subtract_rect(inner);
        assert_eq!(rects, vec![])
    }

    #[test]
    fn rectangle_subtract_center_hole() {
        let outer = Rectangle::<i32, Logical>::from_size((100, 100).into());
        let inner = Rectangle::<i32, Logical>::new((10, 10).into(), (80, 80).into());

        let rects = outer.subtract_rect(inner);
        assert_eq!(
            rects,
            vec![
                // Top rect
                Rectangle::<i32, Logical>::from_size((100, 10).into()),
                // Left rect
                Rectangle::<i32, Logical>::new((0, 10).into(), (10, 80).into()),
                // Right rect
                Rectangle::<i32, Logical>::new((90, 10).into(), (10, 80).into()),
                // Bottom rect
                Rectangle::<i32, Logical>::new((0, 90).into(), (100, 10).into()),
            ]
        )
    }

    #[test]
    fn rectangle_subtract_full_top() {
        let outer = Rectangle::<i32, Logical>::from_size((100, 100).into());
        let inner = Rectangle::<i32, Logical>::new((0, -20).into(), (100, 100).into());

        let rects = outer.subtract_rect(inner);
        assert_eq!(
            rects,
            vec![
                // Bottom rect
                Rectangle::<i32, Logical>::new((0, 80).into(), (100, 20).into()),
            ]
        )
    }

    #[test]
    fn rectangle_subtract_full_bottom() {
        let outer = Rectangle::<i32, Logical>::from_size((100, 100).into());
        let inner = Rectangle::<i32, Logical>::new((0, 20).into(), (100, 100).into());

        let rects = outer.subtract_rect(inner);
        assert_eq!(
            rects,
            vec![
                // Top rect
                Rectangle::<i32, Logical>::from_size((100, 20).into()),
            ]
        )
    }

    #[test]
    fn rectangle_subtract_full_left() {
        let outer = Rectangle::<i32, Logical>::from_size((100, 100).into());
        let inner = Rectangle::<i32, Logical>::new((-20, 0).into(), (100, 100).into());

        let rects = outer.subtract_rect(inner);
        assert_eq!(
            rects,
            vec![
                // Right rect
                Rectangle::<i32, Logical>::new((80, 0).into(), (20, 100).into()),
            ]
        )
    }

    #[test]
    fn rectangle_subtract_full_right() {
        let outer = Rectangle::<i32, Logical>::from_size((100, 100).into());
        let inner = Rectangle::<i32, Logical>::new((20, 0).into(), (100, 100).into());

        let rects = outer.subtract_rect(inner);
        assert_eq!(
            rects,
            vec![
                // Left rect
                Rectangle::<i32, Logical>::from_size((20, 100).into()),
            ]
        )
    }

    #[test]
    fn rectangle_overlaps_or_touches_top() {
        let top = Rectangle::<i32, Logical>::new((0, -24).into(), (800, 24).into());
        let main = Rectangle::<i32, Logical>::from_size((800, 600).into());
        assert!(main.overlaps_or_touches(top));
    }

    #[test]
    fn rectangle_overlaps_or_touches_left() {
        let left = Rectangle::<i32, Logical>::new((-4, -24).into(), (4, 624).into());
        let main = Rectangle::<i32, Logical>::from_size((800, 600).into());
        assert!(main.overlaps_or_touches(left));
    }

    #[test]
    fn rectangle_overlaps_or_touches_right() {
        let right = Rectangle::<i32, Logical>::new((800, -24).into(), (4, 624).into());
        let main = Rectangle::<i32, Logical>::from_size((800, 600).into());
        assert!(main.overlaps_or_touches(right));
    }

    #[test]
    fn rectangle_no_overlap_top() {
        let top = Rectangle::<i32, Logical>::new((0, -24).into(), (800, 24).into());
        let main = Rectangle::<i32, Logical>::from_size((800, 600).into());
        assert!(!main.overlaps(top));
    }

    #[test]
    fn rectangle_no_overlap_left() {
        let left = Rectangle::<i32, Logical>::new((-4, -24).into(), (4, 624).into());
        let main = Rectangle::<i32, Logical>::from_size((800, 600).into());
        assert!(!main.overlaps(left));
    }

    #[test]
    fn rectangle_no_overlap_right() {
        let right = Rectangle::<i32, Logical>::new((800, -24).into(), (4, 624).into());
        let main = Rectangle::<i32, Logical>::from_size((800, 600).into());
        assert!(!main.overlaps(right));
    }

    #[test]
    fn size_sub_saturate_zero() {
        let bigger = Size::<_, Logical>::from((10, 10));
        let mut smaller = Size::<_, Logical>::from((5, 5));

        assert_eq!(smaller - bigger, Size::from((0, 0)));
        smaller -= bigger;
        assert_eq!(smaller, Size::from((0, 0)));
    }
}
