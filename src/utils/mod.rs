//! Various utilities functions and types

mod rectangle;

pub use self::rectangle::Rectangle;

/// This resource is not managed by Smithay
#[derive(Debug)]
pub struct UnmanagedResource;

impl std::fmt::Display for UnmanagedResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("This resource is not managed by Smithay.")
    }
}

impl std::error::Error for UnmanagedResource {}
