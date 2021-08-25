use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};

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

pub trait Coordinate:
    Sized + Add<Self, Output = Self> + Sub<Self, Output = Self> + PartialOrd + Default + Copy + std::fmt::Debug
{
    fn downscale(self, scale: Self) -> Self;
    fn upscale(self, scale: Self) -> Self;
    fn to_f64(self) -> f64;
    fn from_f64(v: f64) -> Self;
    fn non_negative(self) -> bool;
    fn abs(self) -> Self;
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
    pub fn to_buffer(self, scale: N) -> Point<N, Buffer> {
        Point {
            x: self.x.upscale(scale),
            y: self.y.upscale(scale),
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
    pub fn to_logical(self, scale: N) -> Point<N, Logical> {
        Point {
            x: self.x.downscale(scale),
            y: self.y.downscale(scale),
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

impl<N: Add<Output = N>, Kind> Add for Point<N, Kind> {
    type Output = Point<N, Kind>;
    #[inline]
    fn add(self, other: Point<N, Kind>) -> Point<N, Kind> {
        Point {
            x: self.x + other.x,
            y: self.y + other.y,
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: AddAssign, Kind> AddAssign for Point<N, Kind> {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y
    }
}

impl<N: SubAssign, Kind> SubAssign for Point<N, Kind> {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y
    }
}

impl<N: Sub<Output = N>, Kind> Sub for Point<N, Kind> {
    type Output = Point<N, Kind>;
    #[inline]
    fn sub(self, other: Point<N, Kind>) -> Point<N, Kind> {
        Point {
            x: self.x - other.x,
            y: self.y - other.y,
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
    pub fn to_buffer(self, scale: N) -> Size<N, Buffer> {
        Size {
            w: self.w.upscale(scale),
            h: self.h.upscale(scale),
            _kind: std::marker::PhantomData,
        }
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
    pub fn to_logical(self, scale: N) -> Size<N, Logical> {
        Size {
            w: self.w.downscale(scale),
            h: self.h.downscale(scale),
            _kind: std::marker::PhantomData,
        }
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

impl<N: Add<Output = N>, Kind> Add for Size<N, Kind> {
    type Output = Size<N, Kind>;
    #[inline]
    fn add(self, other: Size<N, Kind>) -> Size<N, Kind> {
        Size {
            w: self.w + other.w,
            h: self.h + other.h,
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: AddAssign, Kind> AddAssign for Size<N, Kind> {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.w += rhs.w;
        self.h += rhs.h
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

impl<N: Add<Output = N>, Kind> Add<Size<N, Kind>> for Point<N, Kind> {
    type Output = Point<N, Kind>;
    #[inline]
    fn add(self, other: Size<N, Kind>) -> Point<N, Kind> {
        Point {
            x: self.x + other.w,
            y: self.y + other.h,
            _kind: std::marker::PhantomData,
        }
    }
}

impl<N: Sub<Output = N>, Kind> Sub<Size<N, Kind>> for Point<N, Kind> {
    type Output = Point<N, Kind>;
    #[inline]
    fn sub(self, other: Size<N, Kind>) -> Point<N, Kind> {
        Point {
            x: self.x - other.w,
            y: self.y - other.h,
            _kind: std::marker::PhantomData,
        }
    }
}

/// A rectangle defined by its top-left corner and dimensions
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
    /// Create a new [`Rectangle`] from the coordinates of its top-left corner and its dimensions
    #[inline]
    pub fn from_loc_and_size(loc: impl Into<Point<N, Kind>>, size: impl Into<Size<N, Kind>>) -> Self {
        Rectangle {
            loc: loc.into(),
            size: size.into(),
        }
    }

    /// Create a new [`Rectangle`] from the coordinates of its top-left corner and its dimensions
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
            && (p.x < self.loc.x + self.size.w)
            && (p.y >= self.loc.y)
            && (p.y < self.loc.y + self.size.h)
    }

    /// Checks whether a given [`Rectangle`] overlaps with this one
    #[inline]
    pub fn overlaps(self, other: Rectangle<N, Kind>) -> bool {
        // if the rectangle is not outside of the other
        // they must overlap
        !(
            // self is left of other
            self.loc.x + self.size.w < other.loc.x
            // self is right of other
            ||  self.loc.x > other.loc.x + other.size.w
            // self is above of other
            ||  self.loc.y + self.size.h < other.loc.y
            // self is below of other
            ||  self.loc.y > other.loc.y + other.size.h
        )
    }

    /// Compute the bounding box of a given set of points
    pub fn bounding_box(points: impl IntoIterator<Item = Point<N, Kind>>) -> Self {
        let ret = points.into_iter().fold(None, |acc, point| {
            match acc {
                None => Some((point, point)),
                // we don't have cmp::{min,max} for f64 :(
                Some((min_point, max_point)) => Some((
                    (
                        if min_point.x > point.x {
                            point.x
                        } else {
                            min_point.x
                        },
                        if min_point.y > point.y {
                            point.y
                        } else {
                            min_point.y
                        },
                    )
                        .into(),
                    (
                        if max_point.x < point.x {
                            point.x
                        } else {
                            max_point.x
                        },
                        if max_point.y < point.y {
                            point.y
                        } else {
                            max_point.y
                        },
                    )
                        .into(),
                )),
            }
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
    pub fn to_buffer(self, scale: N) -> Rectangle<N, Buffer> {
        Rectangle {
            loc: self.loc.to_buffer(scale),
            size: self.size.to_buffer(scale),
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
    pub fn to_logical(self, scale: N) -> Rectangle<N, Logical> {
        Rectangle {
            loc: self.loc.to_logical(scale),
            size: self.size.to_logical(scale),
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
