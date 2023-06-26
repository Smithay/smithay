use std::{
    cell::{RefCell, RefMut},
    collections::{HashMap, HashSet},
};

use tracing::{debug, instrument};
use wayland_server::{protocol::wl_surface::WlSurface, Resource, Weak as WlWeak};

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

type OutputSurfacesUserdata = RefCell<HashSet<WlWeak<WlSurface>>>;
fn output_surfaces(o: &Output) -> RefMut<'_, HashSet<WlWeak<WlSurface>>> {
    let userdata = o.user_data();
    userdata.insert_if_missing(OutputSurfacesUserdata::default);
    let mut surfaces = userdata.get::<OutputSurfacesUserdata>().unwrap().borrow_mut();
    surfaces.retain(|s| s.upgrade().is_ok());
    surfaces
}

#[instrument(level = "debug", skip(output), fields(output = output.name()))]
#[profiling::function]
fn output_update(output: &Output, output_overlap: Rectangle<i32, Logical>, surface: &WlSurface) {
    let mut surface_list = output_surfaces(output);

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
                output_leave(output, &mut surface_list, wl_surface);
                return;
            }
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(surface_view) = data.and_then(|d| d.borrow().surface_view) {
                location += surface_view.offset;
                let surface_rectangle = Rectangle::from_loc_and_size(location, surface_view.dst);
                if output_overlap.overlaps(surface_rectangle) {
                    // We found a matching output, check if we already sent enter
                    output_enter(output, &mut surface_list, wl_surface);
                } else {
                    // Surface does not match output, if we sent enter earlier
                    // we should now send leave
                    output_leave(output, &mut surface_list, wl_surface);
                }
            } else {
                // Maybe the the surface got unmapped, send leave on output
                output_leave(output, &mut surface_list, wl_surface);
            }
        },
        |_, _, _| true,
    );
}

fn output_enter(output: &Output, surface_list: &mut HashSet<WlWeak<WlSurface>>, surface: &WlSurface) {
    let weak = surface.downgrade();
    if !surface_list.contains(&weak) {
        debug!("surface entering output",);
        output.enter(surface);
        surface_list.insert(weak);
    }
}

fn output_leave(output: &Output, surface_list: &mut HashSet<WlWeak<WlSurface>>, surface: &WlSurface) {
    let weak = surface.downgrade();
    if surface_list.contains(&weak) {
        debug!("surface leaving output",);
        output.leave(surface);
        surface_list.remove(&weak);
    }
}

#[derive(Debug, Default)]
struct WindowOutputState {
    output_overlap: HashMap<WeakOutput, Rectangle<i32, Logical>>,
}
type WindowOutputUserData = RefCell<WindowOutputState>;
