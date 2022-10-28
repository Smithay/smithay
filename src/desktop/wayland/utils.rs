//! Helper functions to ease dealing with surface trees

use crate::{
    backend::renderer::{
        element::{PrimaryScanoutOutput, RenderElementState, RenderElementStates},
        utils::RendererSurfaceState,
    },
    desktop::WindowSurfaceType,
    output::Output,
    utils::{Logical, Point, Rectangle},
    wayland::compositor::{with_surface_tree_downward, SurfaceAttributes, SurfaceData, TraversalAction},
};
use std::{cell::RefCell, sync::Mutex, time::Duration};
use wayland_server::protocol::wl_surface;

impl RendererSurfaceState {
    fn contains_point<P: Into<Point<f64, Logical>>>(&self, attrs: &SurfaceAttributes, point: P) -> bool {
        let point = point.into();
        let size = match self.surface_view.map(|view| view.dst) {
            None => return false, // If the surface has no size, it can't have an input region.
            Some(size) => size,
        };

        let rect = Rectangle {
            loc: (0, 0).into(),
            size,
        }
        .to_f64();

        // The input region is always within the surface itself, so if the surface itself doesn't contain the
        // point we can return false.
        if !rect.contains(point) {
            return false;
        }

        // If there's no input region, we're done.
        if attrs.input_region.is_none() {
            return true;
        }

        attrs
            .input_region
            .as_ref()
            .unwrap()
            .contains(point.to_i32_round())
    }
}

/// Returns the bounding box of a given surface and all its subsurfaces.
///
/// - `location` can be set to offset the returned bounding box.
pub fn bbox_from_surface_tree<P>(surface: &wl_surface::WlSurface, location: P) -> Rectangle<i32, Logical>
where
    P: Into<Point<i32, Logical>>,
{
    let location = location.into();
    let mut bounding_box = Rectangle::from_loc_and_size(location, (0, 0));
    with_surface_tree_downward(
        surface,
        location,
        |_, states, loc: &Point<i32, Logical>| {
            let mut loc = *loc;
            let data = states.data_map.get::<RefCell<RendererSurfaceState>>();

            if let Some(surface_view) = data.and_then(|d| d.borrow().surface_view) {
                loc += surface_view.offset;
                // Update the bounding box.
                bounding_box = bounding_box.merge(Rectangle::from_loc_and_size(loc, surface_view.dst));

                TraversalAction::DoChildren(loc)
            } else {
                // If the parent surface is unmapped, then the child surfaces are hidden as
                // well, no need to consider them here.
                TraversalAction::SkipChildren
            }
        },
        |_, _, _| {},
        |_, _, _| true,
    );
    bounding_box
}

/// Returns the topmost (sub-)surface under a given position matching the input regions of the surface.
///
/// In case no surface input region matches the point [`None`] is returned.
///
/// - `point` has to be the position to query, relative to (0, 0) of the given surface + `location`.
/// - `location` can be used to offset the returned point.
pub fn under_from_surface_tree<P>(
    surface: &wl_surface::WlSurface,
    point: Point<f64, Logical>,
    location: P,
    surface_type: WindowSurfaceType,
) -> Option<(wl_surface::WlSurface, Point<i32, Logical>)>
where
    P: Into<Point<i32, Logical>>,
{
    let found = RefCell::new(None);
    with_surface_tree_downward(
        surface,
        location.into(),
        |wl_surface, states, location: &Point<i32, Logical>| {
            let mut location = *location;
            let data = states.data_map.get::<RefCell<RendererSurfaceState>>();

            if let Some(surface_view) = data.and_then(|d| d.borrow().surface_view) {
                location += surface_view.offset;

                if states.role == Some("subsurface") || surface_type.contains(WindowSurfaceType::TOPLEVEL) {
                    let contains_the_point = data
                        .map(|data| {
                            data.borrow()
                                .contains_point(&*states.cached_state.current(), point - location.to_f64())
                        })
                        .unwrap_or(false);
                    if contains_the_point {
                        *found.borrow_mut() = Some((wl_surface.clone(), location));
                    }
                }

                if surface_type.contains(WindowSurfaceType::SUBSURFACE) {
                    TraversalAction::DoChildren(location)
                } else {
                    TraversalAction::SkipChildren
                }
            } else {
                // We are completely hidden
                TraversalAction::SkipChildren
            }
        },
        |_, _, _| {},
        |_, _, _| {
            // only continue if the point is not found
            found.borrow().is_none()
        },
    );
    found.into_inner()
}

type SurfacePrimaryScanoutOutput = Mutex<PrimaryScanoutOutput>;

/// Run a closure on all surfaces of a surface tree
pub fn with_surfaces_surface_tree<F>(surface: &wl_surface::WlSurface, mut processor: F)
where
    F: FnMut(&wl_surface::WlSurface, &SurfaceData),
{
    with_surface_tree_downward(
        surface,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |surface, states, _| processor(surface, states),
        |_, _, _| true,
    )
}

/// Retrieve a previously stored primary scan-out output from a surface
///
/// This will always return `None` if [`update_surface_primary_scanout_output`] is not used.
pub fn surface_primary_scanout_output(
    _surface: &wl_surface::WlSurface,
    states: &SurfaceData,
) -> Option<Output> {
    states
        .data_map
        .insert_if_missing_threadsafe(SurfacePrimaryScanoutOutput::default);
    let surface_primary_scanout_output = states.data_map.get::<SurfacePrimaryScanoutOutput>().unwrap();
    surface_primary_scanout_output.lock().unwrap().current_output()
}

/// Update the surface primary scan-out output from a output render report
///
/// The compare function can be used to alter the behavior for selecting the primary scan-out
/// output. See [`update_from_render_element_states`](crate::backend::renderer::element::PrimaryScanoutOutput::update_from_render_element_states) for more information about primary scan-out
/// output selection.
pub fn update_surface_primary_scanout_output<F>(
    surface: &wl_surface::WlSurface,
    output: &Output,
    surface_data: &SurfaceData,
    states: &RenderElementStates,
    compare: F,
) where
    F: for<'a> Fn(&'a Output, &'a RenderElementState, &'a Output, &'a RenderElementState) -> &'a Output,
{
    surface_data
        .data_map
        .insert_if_missing_threadsafe(SurfacePrimaryScanoutOutput::default);
    let surface_primary_scanout_output = surface_data
        .data_map
        .get::<SurfacePrimaryScanoutOutput>()
        .unwrap();
    surface_primary_scanout_output
        .lock()
        .unwrap()
        .update_from_render_element_states(surface, output, states, compare);
}

/// Sends frame callbacks for a surface and its subsurfaces with the given `time`.
///
/// The frame callbacks for a [`WlSurface`](wl_surface::WlSurface) will only be sent if the
/// primary scan-out output equals the provided output or if the surface has no primary
/// scan-out output and the frame callback is overdue. A frame callback is considered
/// overdue if the last time a frame callback has been sent is greater than the provided
/// throttle threshold. If the threshold is `None` this will never send frame callbacks
/// for a surface that is not visible. Specifying [`Duration::ZERO`] as the throttle threshold
/// will always send frame callbacks for non visible surfaces.
#[allow(clippy::too_many_arguments)]
pub fn send_frames_surface_tree<T, F>(
    surface: &wl_surface::WlSurface,
    output: &Output,
    time: T,
    throttle: Option<Duration>,
    mut primary_scan_out_output: F,
) where
    T: Into<Duration>,
    F: FnMut(&wl_surface::WlSurface, &SurfaceData) -> Option<Output>,
{
    let time = time.into();

    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |surface, states, &()| {
            states
                .data_map
                .insert_if_missing_threadsafe(SurfaceFrameThrottlingState::default);
            let surface_frame_throttling_state =
                states.data_map.get::<SurfaceFrameThrottlingState>().unwrap();

            let on_primary_scanout_output = primary_scan_out_output(surface, states)
                .map(|preferred_output| preferred_output == *output)
                .unwrap_or(false);

            let frame_overdue = surface_frame_throttling_state.update(time, throttle);

            // We only want to send frame callbacks on the primary scan-out output
            // or if we have no output and the frame is overdue, this can only
            // happen if the surface is completely occluded on all outputs
            let send_frame_callback = on_primary_scanout_output || frame_overdue;

            if send_frame_callback {
                // the surface may not have any user_data if it is a subsurface and has not
                // yet been commited
                for callback in states
                    .cached_state
                    .current::<SurfaceAttributes>()
                    .frame_callbacks
                    .drain(..)
                {
                    callback.done(time.as_millis() as u32);
                }
            }
        },
        |_, _, &()| true,
    );
}

#[derive(Debug, Default)]
struct SurfaceFrameThrottlingState(Mutex<Option<Duration>>);

impl SurfaceFrameThrottlingState {
    pub fn update(&self, time: Duration, throttle: Option<Duration>) -> bool {
        if let Some(throttle) = throttle {
            let mut guard = self.0.lock().unwrap();
            let send_throttled_frame = guard
                .map(|last| time.saturating_sub(last) > throttle)
                .unwrap_or(true);
            if send_throttled_frame {
                *guard = Some(time);
            }
            send_throttled_frame
        } else {
            false
        }
    }
}
