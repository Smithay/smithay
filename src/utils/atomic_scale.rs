
//! Atomic scale type backed by 64-bit atomic on platform supporting it, falling-back to 32-bit storage on unsupported platforms.

use std::sync::atomic::Ordering;

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
