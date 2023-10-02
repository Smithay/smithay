#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::wl_surface::WlSurface;

pub use cursor_icon::CursorIcon;

use crate::utils::{Logical, Point};
use std::sync::Mutex;

/// The role representing a surface set as the pointer cursor
#[derive(Debug, Default, Copy, Clone)]
pub struct CursorImageAttributes {
    /// Location of the hotspot of the pointer in the surface
    pub hotspot: Point<i32, Logical>,
}

/// Data associated with XDG toplevel surface  
///
/// ```no_run
/// # #[cfg(feature = "wayland_frontend")]
/// use smithay::wayland::compositor;
/// use smithay::input::pointer::CursorImageSurfaceData;
///
/// # let wl_surface = todo!();
/// # #[cfg(feature = "wayland_frontend")]
/// compositor::with_states(&wl_surface, |states| {
///     states.data_map.get::<CursorImageSurfaceData>();
/// });
/// ```
pub type CursorImageSurfaceData = Mutex<CursorImageAttributes>;

/// Possible status of a cursor as requested by clients
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorImageStatus {
    /// The cursor should be hidden
    Hidden,
    /// The compositor should draw the given named
    /// cursor.
    Named(CursorIcon),
    // TODO bitmap, dmabuf cursor? Or let the compositor handle everything through "Default"
    /// The cursor should be drawn using this surface as an image
    #[cfg(feature = "wayland_frontend")]
    Surface(WlSurface),
}

impl CursorImageStatus {
    /// Get default `Named` cursor.
    pub fn default_named() -> Self {
        Self::Named(CursorIcon::Default)
    }
}
