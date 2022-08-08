use wayland_server::protocol::wl_surface::WlSurface;

use crate::utils::{Logical, Point};

/// The role representing a surface set as the pointer cursor
#[derive(Debug, Default, Copy, Clone)]
pub struct CursorImageAttributes {
    /// Location of the hotspot of the pointer in the surface
    pub hotspot: Point<i32, Logical>,
}

/// Possible status of a cursor as requested by clients
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorImageStatus {
    /// The cursor should be hidden
    Hidden,
    /// The compositor should draw its cursor
    Default,

    // TODO bitmap, dmabuf cursor? Or let the compositor handle everything through "Default"
    /// The cursor should be drawn using this surface as an image
    #[cfg(feature = "wayland_frontend")]
    Surface(WlSurface),
}
