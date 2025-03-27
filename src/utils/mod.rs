//! Various utilities functions and types

mod geometry;
pub mod signaling;

#[cfg(feature = "x11rb_event_source")]
pub mod x11rb;

pub(crate) mod ids;
pub mod user_data;

pub(crate) mod alive_tracker;
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
