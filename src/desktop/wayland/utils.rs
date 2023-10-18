//! Helper functions to ease dealing with surface trees

use crate::{
    backend::renderer::{
        element::{
            PrimaryScanoutOutput, RenderElementPresentationState, RenderElementState, RenderElementStates,
        },
        utils::RendererSurfaceState,
    },
    desktop::WindowSurfaceType,
    output::{Output, WeakOutput},
    utils::{Logical, Point, Rectangle, Time},
    wayland::{
        compositor::{with_surface_tree_downward, SurfaceAttributes, SurfaceData, TraversalAction},
        dmabuf::{DmabufFeedback, SurfaceDmabufFeedbackState},
        presentation::{PresentationFeedbackCachedState, PresentationFeedbackCallback},
    },
};
use std::{cell::RefCell, sync::Mutex, time::Duration};
use wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use wayland_server::protocol::wl_surface;

pub use super::super::space::wayland::output_update;

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
                                .contains_point(&states.cached_state.current(), point - location.to_f64())
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
) -> Option<Output>
where
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
        .update_from_render_element_states(surface, output, states, compare)
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

/// Sends dmabuf feedback for a surface and its subsurfaces with the given select function.
///
/// The dmabuf feedback for a [`WlSurface`](wl_surface::WlSurface) will only be sent if the
/// primary scan-out output equals the provided output and the surface has requested dmabuf
/// feedback.
pub fn send_dmabuf_feedback_surface_tree<'a, P, F>(
    surface: &wl_surface::WlSurface,
    output: &Output,
    mut primary_scan_out_output: P,
    select_dmabuf_feedback: F,
) where
    P: FnMut(&wl_surface::WlSurface, &SurfaceData) -> Option<Output>,
    F: Fn(&wl_surface::WlSurface, &SurfaceData) -> &'a DmabufFeedback,
{
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |surface, states, &()| {
            let on_primary_scanout_output = primary_scan_out_output(surface, states)
                .map(|preferred_output| preferred_output == *output)
                .unwrap_or(false);

            if !on_primary_scanout_output {
                return;
            }

            let Some(surface_feedback) = SurfaceDmabufFeedbackState::from_states(states) else {
                return;
            };

            let feedback = select_dmabuf_feedback(surface, states);
            surface_feedback.set_feedback(feedback);
        },
        |_, _, &()| true,
    );
}

/// Holds the presentation feedback for a surface
#[derive(Debug)]
pub struct SurfacePresentationFeedback {
    callbacks: Vec<PresentationFeedbackCallback>,
    flags: wp_presentation_feedback::Kind,
}

impl SurfacePresentationFeedback {
    /// Create a [`SurfacePresentationFeedback`] from the surface states.
    ///
    /// Returns `None` if the surface has no stored presentation feedback
    pub fn from_states(states: &SurfaceData, flags: wp_presentation_feedback::Kind) -> Option<Self> {
        let mut presentation_feedback_state =
            states.cached_state.current::<PresentationFeedbackCachedState>();
        if presentation_feedback_state.callbacks.is_empty() {
            return None;
        }

        let callbacks = std::mem::take(&mut presentation_feedback_state.callbacks);
        Some(SurfacePresentationFeedback { callbacks, flags })
    }

    /// Mark the presentation feedbacks for this surface as presented
    ///
    /// If the passed in clk_id does not match the clk_id of a stored
    /// presentation feedback the feedback will be discarded.
    pub fn presented(
        &mut self,
        output: &Output,
        clk_id: u32,
        time: impl Into<Duration>,
        refresh: impl Into<Duration>,
        seq: u64,
        flags: wp_presentation_feedback::Kind,
    ) {
        let time = time.into();
        let refresh = refresh.into();

        for callback in self.callbacks.drain(..) {
            if callback.clk_id() == clk_id {
                callback.presented(output, time, refresh, seq, flags | self.flags)
            } else {
                callback.discarded()
            }
        }
    }

    /// Mark the presentation feedbacks for this surface as discarded
    pub fn discarded(&mut self) {
        for callback in self.callbacks.drain(..) {
            callback.discarded()
        }
    }
}

impl Drop for SurfacePresentationFeedback {
    fn drop(&mut self) {
        self.discarded()
    }
}

/// Stores the [`SurfacePresentationFeedback`] for a specific output
///
/// This is intended to be used in combination with [`take_presentation_feedback_surface_tree`].
#[derive(Debug)]
pub struct OutputPresentationFeedback {
    output: WeakOutput,
    callbacks: Vec<SurfacePresentationFeedback>,
}

impl OutputPresentationFeedback {
    /// Create a new [`OutputPresentationFeedback`] for a specific [`Output`]
    pub fn new(output: &Output) -> Self {
        OutputPresentationFeedback {
            output: output.downgrade(),
            callbacks: Vec::new(),
        }
    }

    /// Returns the associated output
    ///
    /// Returns `None` if the output has been destroyed.
    pub fn output(&self) -> Option<Output> {
        self.output.upgrade()
    }

    /// Mark all stored [`SurfacePresentationFeedback`]s as presented
    ///
    /// The flags passed to this function will be combined with the stored
    /// per surface flags.
    pub fn presented<T, Kind>(
        &mut self,
        time: T,
        refresh: impl Into<Duration>,
        seq: u64,
        flags: wp_presentation_feedback::Kind,
    ) where
        T: Into<Time<Kind>>,
        Kind: crate::utils::NonNegativeClockSource,
    {
        let time = time.into();
        let refresh = refresh.into();
        let clk_id = Kind::ID as u32;
        if let Some(output) = self.output.upgrade() {
            for mut callback in self.callbacks.drain(..) {
                callback.presented(&output, clk_id, time, refresh, seq, flags);
            }
        } else {
            self.discarded();
        }
    }

    /// Mark all stored [`SurfacePresentationFeedback`]s as discarded
    pub fn discarded(&mut self) {
        for mut callback in self.callbacks.drain(..) {
            callback.discarded();
        }
    }
}

/// Takes the [`PresentationFeedbackCallback`]s from the surface tree
///
/// This moves the [`PresentationFeedbackCallback`]s from the surfaces
/// where the primary scan-out matches the output of the [`OutputPresentationFeedback`]
/// to the [`OutputPresentationFeedback`]
///
/// The flags closure can be used to set special flags per surface like [`wp_presentation_feedback::Kind::ZeroCopy`]
pub fn take_presentation_feedback_surface_tree<F1, F2>(
    surface: &wl_surface::WlSurface,
    output_feedback: &mut OutputPresentationFeedback,
    mut primary_scan_out_output: F1,
    mut presentation_feedback_flags: F2,
) where
    F1: FnMut(&wl_surface::WlSurface, &SurfaceData) -> Option<Output>,
    F2: FnMut(&wl_surface::WlSurface, &SurfaceData) -> wp_presentation_feedback::Kind,
{
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |surface, states, &()| {
            let on_primary_scanout_output = primary_scan_out_output(surface, states)
                .map(|preferred_output| preferred_output == output_feedback.output)
                .unwrap_or(false);

            if !on_primary_scanout_output {
                return;
            }

            let flags = presentation_feedback_flags(surface, states);
            if let Some(feedback) = SurfacePresentationFeedback::from_states(states, flags) {
                output_feedback.callbacks.push(feedback);
            }
        },
        |_, _, &()| true,
    );
}

/// Retrieves the per surface [`wp_presentation_feedback::Kind`] flags
///
/// This will return [`wp_presentation_feedback::Kind::ZeroCopy`] if the surface
/// has been presented using zero-copy according to the [`RenderElementState`]
/// in the provided [`RenderElementStates`]
pub fn surface_presentation_feedback_flags_from_states(
    surface: &wl_surface::WlSurface,
    states: &RenderElementStates,
) -> wp_presentation_feedback::Kind {
    let zero_copy = states
        .element_render_state(surface)
        .map(|state| state.presentation_state == RenderElementPresentationState::ZeroCopy)
        .unwrap_or(false);

    if zero_copy {
        wp_presentation_feedback::Kind::ZeroCopy
    } else {
        wp_presentation_feedback::Kind::empty()
    }
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
