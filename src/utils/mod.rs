//! Various utilities functions and types

mod geometry;
pub mod signaling;

#[cfg(feature = "x11rb_event_source")]
pub mod x11rb;

pub(crate) mod ids;
pub mod user_data;

pub(crate) mod alive_tracker;
pub use self::alive_tracker::IsAlive;

pub use self::geometry::{
    Buffer, Coordinate, Logical, Physical, Point, Raw, Rectangle, Scale, Size, Transform,
};

mod serial;
pub use serial::*;

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
