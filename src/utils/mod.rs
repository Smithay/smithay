//! Various utilities functions and types

mod geometry;
pub mod signaling;

#[cfg(feature = "x11rb_event_source")]
pub mod x11rb;

pub(crate) mod ids;
pub mod user_data;

pub(crate) mod alive_tracker;
use std::sync::atomic::Ordering;

pub use self::alive_tracker::IsAlive;

#[cfg(feature = "wayland_frontend")]
pub(crate) mod iter;

mod fd;
pub use fd::*;

mod sealed_file;
pub use sealed_file::SealedFile;

#[cfg(feature = "wayland_frontend")]
pub(crate) use self::geometry::Client;
pub use self::geometry::{
    Buffer, Coordinate, Logical, Physical, Point, Raw, Rectangle, Scale, Size, Transform,
};

mod serial;
pub use serial::*;

mod clock;
pub use clock::*;

#[cfg(feature = "wayland_frontend")]
pub(crate) mod hook;
#[cfg(feature = "wayland_frontend")]
pub use hook::HookId;

/// This resource is not managed by Smithay
#[derive(Debug)]
pub struct UnmanagedResource;

impl std::fmt::Display for UnmanagedResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("This resource is not managed by Smithay.")
    }
}

impl std::error::Error for UnmanagedResource {}

/// This resource has been destroyed and can no longer be used.
#[derive(Debug)]
pub struct DeadResource;

impl std::fmt::Display for DeadResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("This resource has been destroyed and can no longer be used.")
    }
}

impl std::error::Error for DeadResource {}

#[cfg(target_has_atomic = "64")]
type AtomicFScaleValue = f64;
#[cfg(target_has_atomic = "64")]
type AtomicFScaleInner = atomic_float::AtomicF64;

#[cfg(not(target_has_atomic = "64"))]
type AtomicFScaleValue = f32;

#[cfg(not(target_has_atomic = "64"))]
type AtomicFScaleInner = atomic_float::AtomicF32;

/// Wrapper about either [`AtomicF64`](atomic_float::AtomicF64) or [`AtomicF32`](atomic_float::AtomicF32)
///
/// On platforms supporting 64-bit atomics, this acts exactly like
/// [`AtomicF64`](atomic_float::AtomicF64) would.
/// 
/// On platforms having only 32-bit ones, this still has API with [`f64`],
/// but uses [`AtomicF32`](atomic_float::AtomicF32) internally and
/// storing operations will lead to precision loss.
#[derive(Debug)]
pub struct AtomicFScale(AtomicFScaleInner);
impl AtomicFScale {
    /// Loads a value from the atomic float.
    ///
    /// On platforms lacking 64-bit atomic support, this will widen the float.
    #[inline]
    pub fn load(&self, ordering: Ordering) -> f64 {
        self.0.load(ordering).into()
    }

    /// Initialize atomic from value
    ///
    /// On platforms lacking 64-bit atomic supports, this means precision loss.
    #[inline]
    pub fn new(val: f64) -> Self {
        Self((val as AtomicFScaleValue).into())
    }

    /// Stores a value into the atomic float, returning the previous value.
    ///
    /// On platforms lacking 64-bit atomic supports, this means precision loss.
    #[inline]
    pub fn swap(&self, new_value: f64, ordering: Ordering) -> f64 {
        self.0.swap(new_value as AtomicFScaleValue, ordering).into()
    }

    /// Store a value into the atomic float.
    ///
    /// On platforms lacking 64-bit atomic supports, this means precision loss.
    #[inline]
    pub fn store(&self, new_value: f64, ordering: Ordering) {
        self.0.store(new_value as AtomicFScaleValue, ordering);
    }
}
