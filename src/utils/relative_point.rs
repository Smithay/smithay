#[cfg(feature = "wayland_frontend")]
use crate::wayland::seat::WaylandFocus;

use super::{Logical, Point};

/// Represents a point on a surface, relative to that surface's origin in
/// global compositor space.
#[derive(Debug, Clone)]
pub struct RelativePoint<S> {
    /// The surface that the pointer is over.
    pub surface: S,
    /// The relative location of the cursor within the surface.
    pub loc: Point<f64, Logical>,
}

#[cfg(feature = "wayland_frontend")]
impl<S: WaylandFocus> RelativePoint<S> {
    /// Calculates the relative position of a cursor on a target surface,
    /// given a point and that  surface's origin coordinates, both in global
    /// compositor space.
    pub fn on_focused_surface(
        point: Point<f64, Logical>,
        focus: S,
        focus_origin: impl Into<Point<i32, Logical>>,
    ) -> Self {
        Self {
            surface: focus,
            loc: point - focus_origin.into().to_f64(),
        }
    }
}
