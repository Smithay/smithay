use std::{cell::RefCell, collections::HashMap};

use tracing::instrument;
use wayland_server::protocol::wl_surface::WlSurface;

use crate::{
    backend::renderer::utils::RendererSurfaceStateUserData,
    output::{Output, WeakOutput},
    utils::{Logical, Point, Rectangle},
    wayland::compositor::{with_surface_tree_downward, TraversalAction},
};

mod layer;
mod window;
#[cfg(feature = "xwayland")]
mod x11;

/// Updates the output overlap for a surface tree.
///
/// Surfaces in the tree will receive output enter and leave events as necessary according to their
/// computed overlap.
#[instrument(level = "trace", skip(output), fields(output = output.name()))]
#[profiling::function]
pub fn output_update(output: &Output, output_overlap: Option<Rectangle<i32, Logical>>, surface: &WlSurface) {
    with_surface_tree_downward(
        surface,
        (Point::from((0, 0)), false),
        |_, states, (location, parent_unmapped)| {
            let mut location = *location;
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            // If the parent is unmapped we still have to traverse
            // our children to send a leave events
            if *parent_unmapped {
                TraversalAction::DoChildren((location, true))
            } else if let Some(surface_view) = data.and_then(|d| d.borrow().surface_view) {
                location += surface_view.offset;
                TraversalAction::DoChildren((location, false))
            } else {
                // If we are unmapped we still have to traverse
                // our children to send leave events
                TraversalAction::DoChildren((location, true))
            }
        },
        |wl_surface, states, (location, parent_unmapped)| {
            let mut location = *location;

            if *parent_unmapped {
                // The parent is unmapped, just send a leave event
                // if we were previously mapped and exit early
                output.leave(wl_surface);
                return;
            }

            let Some(output_overlap) = output_overlap else {
                // There's no overlap, send a leave event.
                output.leave(wl_surface);
                return;
            };

            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(surface_view) = data.and_then(|d| d.borrow().surface_view) {
                location += surface_view.offset;
                let surface_rectangle = Rectangle::from_loc_and_size(location, surface_view.dst);
                if output_overlap.overlaps(surface_rectangle) {
                    // We found a matching output, check if we already sent enter
                    output.enter(wl_surface);
                } else {
                    // Surface does not match output, if we sent enter earlier
                    // we should now send leave
                    output.leave(wl_surface);
                }
            } else {
                // Maybe the the surface got unmapped, send leave on output
                output.leave(wl_surface);
            }
        },
        |_, _, _| true,
    );
}

#[derive(Debug, Default)]
struct WindowOutputState {
    output_overlap: HashMap<WeakOutput, Rectangle<i32, Logical>>,
}
type WindowOutputUserData = RefCell<WindowOutputState>;
