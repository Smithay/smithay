//! Utilities for handling surfaces, subsurfaces and regions
//!
//! This module provides automatic handling of surfaces, subsurfaces
//! and region Wayland objects, by registering an implementation for
//! for the [`wl_compositor`](wayland_server::protocol::wl_compositor)
//! and [`wl_subcompositor`](wayland_server::protocol::wl_subcompositor) globals.
//!
//! ## Why use this implementation
//!
//! This implementation does a simple job: it stores in a coherent way the state of
//! surface trees with subsurfaces, to provide you direct access to the tree
//! structure and all surface attributes, and handles the application of double-buffered
//! state.
//!
//! As such, you can, given a root surface with a role requiring it to be displayed,
//! you can iterate over the whole tree of subsurfaces to recover all the metadata you
//! need to display the subsurface tree.
//!
//! This implementation will not do anything more than present you the metadata specified by the
//! client in a coherent and practical way. All the logic regarding drawing itself, and
//! the positioning of windows (surface trees) one relative to another is out of its scope.
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this implementation, use the [`compositor_init`]
//! method provided by this module. It'll require you to first define as few things, as shown in
//! this example:
//!
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! use smithay::wayland::compositor::compositor_init;
//!
//! # let mut display = wayland_server::Display::new();
//! // Call the init function:
//! compositor_init(
//!     &mut display,
//!     |surface, dispatch_data| {
//!         /*
//!           Your handling of surface commits
//!          */
//!     },
//!     None // put a logger here
//! );
//!
//! // You're now ready to go!
//! ```
//!
//! ### Use the surface states
//!
//! The main access to surface states is done through the [`with_states`] function, which
//! gives you access to the [`SurfaceData`] instance associated with this surface. It acts
//! as a general purpose container for associating state to a surface, double-buffered or
//! not. See its documentation for more details.
//!
//! ### State application and hooks
//!
//! On commit of a surface several steps are taken to update the state of the surface. Actions
//! are taken by smithay in the following order:
//!
//! 1. Commit hooks registered to this surface are invoked. Such hooks can be registered using
//!    the [`add_commit_hook`] function. They are typically used by protocol extensions that
//!    add state to a surface and need to check on commit that client did not request an
//!    illegal state before it is applied on commit.
//! 2. The pending state is either applied and made current, or cached for later application
//!    is the surface is a synchronize subsurface. If the current state is applied, state
//!    of the synchronized children subsurface are applied as well at this point.
//! 3. Your user callback provided to [`compositor_init`] is invoked, so that you can access
//!    the new current state of the surface. The state of sync children subsurfaces of your
//!    surface may have changed as well, so this is the place to check it, using functions
//!    like [`with_surface_tree_upward`] or [`with_surface_tree_downward`]. On the other hand,
//!    if the surface is a sync subsurface, its current state will note have changed as
//!    the result of that commit. You can check if it is using [`is_sync_subsurface`].
//!
//! ### Surface roles
//!
//! The wayland protocol specifies that a surface needs to be assigned a role before it can
//! be displayed. Furthermore, a surface can only have a single role during its whole lifetime.
//! Smithay represents this role as a `&'static str` identifier, that can only be set once
//! on a surface. See [`give_role`] and [`get_role`] for details. This module manages the
//! subsurface role, which is identified by the string `"subsurface"`.

mod cache;
mod handlers;
mod transaction;
mod tree;

use std::marker::PhantomData;

pub use self::cache::{Cacheable, MultiCache};
pub use self::handlers::SubsurfaceCachedState;
use self::tree::PrivateSurfaceData;
pub use self::tree::{AlreadyHasRole, TraversalAction};
use crate::utils::{user_data::UserDataMap, Buffer, DeadResource, Logical, Point, Rectangle};
use wayland_server::backend::GlobalId;
use wayland_server::protocol::wl_compositor::WlCompositor;
use wayland_server::protocol::wl_subcompositor::WlSubcompositor;
use wayland_server::protocol::{wl_buffer, wl_callback, wl_output, wl_region, wl_surface::WlSurface};
use wayland_server::{DisplayHandle, GlobalDispatch, Resource};

pub use handlers::{RegionUserData, SubsurfaceUserData, SurfaceUserData};

/// Description of a part of a surface that
/// should be considered damaged and needs to be redrawn
#[derive(Debug)]
pub enum Damage {
    /// A rectangle containing the damaged zone, in surface coordinates
    Surface(Rectangle<i32, Logical>),
    /// A rectangle containing the damaged zone, in buffer coordinates
    ///
    /// Note: Buffer scaling must be taken into consideration
    Buffer(Rectangle<i32, Buffer>),
}

#[derive(Debug, Copy, Clone, Default)]
struct Marker<R> {
    _r: ::std::marker::PhantomData<R>,
}

/// The state container associated with a surface
///
/// This general-purpose container provides 2 main storages:
///
/// - the `data_map` storage has typemap semantics and allows you
///   to associate and access non-buffered data to the surface
/// - the `cached_state` storages allows you to associate state to
///   the surface that follows the double-buffering semantics associated
///   with the `commit` procedure of surfaces, also with typemap-like
///   semantics
///
/// See the respective documentation of each container for its usage.
///
/// By default, all surfaces have a [`SurfaceAttributes`] cached state,
/// and subsurface also have a [`SubsurfaceCachedState`] state as well.
#[derive(Debug)]
pub struct SurfaceData<D> {
    /// The current role of the surface.
    ///
    /// If `None` if the surface has not yet been assigned a role
    pub role: Option<&'static str>,
    /// The non-buffered typemap storage of this surface
    pub data_map: UserDataMap,
    /// The double-buffered typemap storage of this surface
    pub cached_state: MultiCache<D>,
}

/// New buffer assignation for a surface
#[derive(Debug)]
pub enum BufferAssignment {
    /// The surface no longer has a buffer attached to it
    Removed,
    /// A new buffer has been attached
    NewBuffer {
        /// The buffer object
        buffer: wl_buffer::WlBuffer,
        /// location of the new buffer relative to the previous one
        delta: Point<i32, Logical>,
    },
}

/// General state associated with a surface
///
/// The fields `buffer`, `damage` and `frame_callbacks` should be
/// reset (by clearing their contents) once you have adequately
/// processed them, as their contents are aggregated from commit to commit.
#[derive(Debug)]
pub struct SurfaceAttributes {
    /// Buffer defining the contents of the surface
    ///
    /// You are free to set this field to `None` to avoid processing it several
    /// times. It'll be set to `Some(...)` if the user attaches a buffer (or `NULL`) to
    /// the surface, and be left to `None` if the user does not attach anything.
    pub buffer: Option<BufferAssignment>,
    /// Scale of the contents of the buffer, for higher-resolution contents.
    ///
    /// If it matches the one of the output displaying this surface, no change
    /// is necessary.
    pub buffer_scale: i32,
    /// Transform under which interpret the contents of the buffer
    ///
    /// If it matches the one of the output displaying this surface, no change
    /// is necessary.
    pub buffer_transform: wl_output::Transform,
    /// Region of the surface that is guaranteed to be opaque
    ///
    /// By default the whole surface is potentially transparent
    pub opaque_region: Option<RegionAttributes>,
    /// Region of the surface that is sensitive to user input
    ///
    /// By default the whole surface should be sensitive
    pub input_region: Option<RegionAttributes>,
    /// Damage rectangle
    ///
    /// Hint provided by the client to suggest that only this part
    /// of the surface was changed and needs to be redrawn
    pub damage: Vec<Damage>,
    /// The frame callbacks associated with this surface for the commit
    ///
    /// The server must send the notifications so that a client
    /// will not send excessive updates, while still allowing
    /// the highest possible update rate for clients that wait for the reply
    /// before drawing again. The server should give some time for the client
    /// to draw and commit after sending the frame callback events to let it
    /// hit the next output refresh.
    ///
    /// A server should avoid signaling the frame callbacks if the
    /// surface is not visible in any way, e.g. the surface is off-screen,
    /// or completely obscured by other opaque surfaces.
    ///
    /// An example possibility would be to trigger it once the frame
    /// associated with this commit has been displayed on the screen.
    pub frame_callbacks: Vec<wl_callback::WlCallback>,
}

impl Default for SurfaceAttributes {
    fn default() -> SurfaceAttributes {
        SurfaceAttributes {
            buffer: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            opaque_region: None,
            input_region: None,
            damage: Vec::new(),
            frame_callbacks: Vec::new(),
        }
    }
}

/// Kind of a rectangle part of a region
#[derive(Copy, Clone, Debug)]
pub enum RectangleKind {
    /// This rectangle should be added to the region
    Add,
    /// The intersection of this rectangle with the region should
    /// be removed from the region
    Subtract,
}

/// Description of the contents of a region
///
/// A region is defined as an union and difference of rectangle.
///
/// This struct contains an ordered `Vec` containing the rectangles defining
/// a region. They should be added or subtracted in this order to compute the
/// actual contents of the region.
#[derive(Clone, Debug, Default)]
pub struct RegionAttributes {
    /// List of rectangle part of this region
    pub rects: Vec<(RectangleKind, Rectangle<i32, Logical>)>,
}

impl RegionAttributes {
    /// Checks whether given point is inside the region.
    pub fn contains<P: Into<Point<i32, Logical>>>(&self, point: P) -> bool {
        let point: Point<i32, Logical> = point.into();
        let mut contains = false;
        for (kind, rect) in &self.rects {
            if rect.contains(point) {
                match kind {
                    RectangleKind::Add => contains = true,
                    RectangleKind::Subtract => contains = false,
                }
            }
        }
        contains
    }
}

/// Access the data of a surface tree from bottom to top
///
/// You provide three closures, a "filter", a "processor" and a "post filter".
///
/// The first closure is initially called on a surface to determine if its children
/// should be processed as well. It returns a `TraversalAction<T>` reflecting that.
///
/// The second closure is supposed to do the actual processing. The processing closure for
/// a surface may be called after the processing closure of some of its children, depending
/// on the stack ordering the client requested. Here the surfaces are processed in the same
/// order as they are supposed to be drawn: from the farthest of the screen to the nearest.
///
/// The third closure is called once all the subtree of a node has been processed, and gives
/// an opportunity for early-stopping. If it returns `true` the processing will continue,
/// while if it returns `false` it'll stop.
///
/// The arguments provided to the closures are, in this order:
///
/// - The surface object itself
/// - a mutable reference to its surface attribute data
/// - a mutable reference to its role data,
/// - a custom value that is passed in a fold-like manner, but only from the output of a parent
///   to its children. See [`TraversalAction`] for details.
///
/// If the surface not managed by the `CompositorGlobal` that provided this token, this
/// will panic (having more than one compositor is not supported).
pub fn with_surface_tree_upward<D, F1, F2, F3, T>(
    surface: &WlSurface,
    initial: T,
    filter: F1,
    processor: F2,
    post_filter: F3,
) where
    D: 'static,
    F1: FnMut(&WlSurface, &SurfaceData<D>, &T) -> TraversalAction<T>,
    F2: FnMut(&WlSurface, &SurfaceData<D>, &T),
    F3: FnMut(&WlSurface, &SurfaceData<D>, &T) -> bool,
{
    PrivateSurfaceData::map_tree(surface, &initial, filter, processor, post_filter, false);
}

/// Access the data of a surface tree from top to bottom
///
/// Behavior is the same as [`with_surface_tree_upward`], but the processing is done in the reverse order,
/// from the nearest of the screen to the deepest.
///
/// This would typically be used to find out which surface of a subsurface tree has been clicked for example.
pub fn with_surface_tree_downward<D, F1, F2, F3, T>(
    surface: &WlSurface,
    initial: T,
    filter: F1,
    processor: F2,
    post_filter: F3,
) where
    D: 'static,
    F1: FnMut(&WlSurface, &SurfaceData<D>, &T) -> TraversalAction<T>,
    F2: FnMut(&WlSurface, &SurfaceData<D>, &T),
    F3: FnMut(&WlSurface, &SurfaceData<D>, &T) -> bool,
{
    PrivateSurfaceData::map_tree(surface, &initial, filter, processor, post_filter, true);
}

/// Retrieve the parent of this surface
///
/// Returns `None` is this surface is a root surface
pub fn get_parent<D: 'static>(surface: &WlSurface) -> Option<WlSurface> {
    // TODO:
    // if !surface.as_ref().is_alive() {
    //     return None;
    // }
    PrivateSurfaceData::<D>::get_parent(surface)
}

/// Retrieve the children of this surface
pub fn get_children<D: 'static>(surface: &WlSurface) -> Vec<WlSurface> {
    // TODO:
    // if !surface.as_ref().is_alive() {
    //     return Vec::new();
    // }
    PrivateSurfaceData::<D>::get_children(surface)
}

/// Check if this subsurface is a synchronized subsurface
///
/// Returns false if the surface is already dead
pub fn is_sync_subsurface<D: 'static>(surface: &WlSurface) -> bool {
    // TODO:
    // if !surface.as_ref().is_alive() {
    //     return false;
    // }
    self::handlers::is_effectively_sync::<D>(surface)
}

/// Get the current role of this surface
pub fn get_role<D: 'static>(surface: &WlSurface) -> Option<&'static str> {
    // TODO:
    // if !surface.as_ref().is_alive() {
    //     return None;
    // }
    PrivateSurfaceData::<D>::get_role(surface)
}

/// Register that this surface has given role
///
/// Fails if the surface already has a role.
pub fn give_role<D: 'static>(surface: &WlSurface, role: &'static str) -> Result<(), AlreadyHasRole> {
    // TODO:
    // if !surface.as_ref().is_alive() {
    //     return Ok(());
    // }
    PrivateSurfaceData::<D>::set_role(surface, role)
}

/// Access the states associated to this surface
pub fn with_states<D, F, T>(surface: &WlSurface, f: F) -> Result<T, DeadResource>
where
    D: 'static,
    F: FnOnce(&SurfaceData<D>) -> T,
{
    // TODO:
    // if !surface.as_ref().is_alive() {
    //     return Err(DeadResource);
    // }
    Ok(PrivateSurfaceData::with_states(surface, f))
}

/// Retrieve the metadata associated with a `wl_region`
///
/// If the region is not managed by the `CompositorGlobal` that provided this token, this
/// will panic (having more than one compositor is not supported).
pub fn get_region_attributes(region: &wl_region::WlRegion) -> RegionAttributes {
    match region.data::<RegionUserData>() {
        Some(data) => data.inner.lock().unwrap().clone(),
        None => panic!("Accessing the data of foreign regions is not supported."),
    }
}

/// Register a commit hook to be invoked on surface commit
///
/// For its precise semantics, see module-level documentation.
pub fn add_commit_hook<D: 'static>(surface: &WlSurface, hook: fn(&WlSurface)) {
    // TODO:
    // if !surface.as_ref().is_alive() {
    //     return;
    // }
    PrivateSurfaceData::<D>::add_commit_hook(surface, hook)
}

/// Handler trait for compositor
pub trait CompositorHandler<D> {
    /// Surface commit handler
    fn commit(&mut self, cx: &mut DisplayHandle<'_, D>, surface: &WlSurface);
}

/// Compositor event dispatching struct
#[derive(Debug)]
pub struct CompositorDispatch<'a, D, H: CompositorHandler<D>>(pub &'a mut CompositorState<D>, pub &'a mut H);

/// State of a compositor
#[derive(Debug)]
pub struct CompositorState<D> {
    log: slog::Logger,
    compositor: GlobalId,
    subcompositor: GlobalId,
    _pd: PhantomData<D>,
}

impl<D> CompositorState<D> {
    /// Create new [`wl_compositor`](wayland_server::protocol::wl_compositor)
    /// and [`wl_subcompositor`](wayland_server::protocol::wl_subcompositor) globals.
    ///
    /// It returns the two global handles, in case you wish to remove these globals from
    /// the event loop in the future.
    pub fn new<L>(display: &mut DisplayHandle<'_, D>, logger: L) -> Self
    where
        L: Into<Option<::slog::Logger>>,
        D: GlobalDispatch<WlCompositor, GlobalData = ()>
            + GlobalDispatch<WlSubcompositor, GlobalData = ()>
            + 'static,
    {
        let log = crate::slog_or_fallback(logger).new(slog::o!("smithay_module" => "compositor_handler"));

        let compositor = display.create_global::<WlCompositor>(4, ());
        let subcompositor = display.create_global::<WlSubcompositor>(1, ());

        CompositorState {
            log,
            compositor,
            subcompositor,
            _pd: PhantomData::<D>,
        }
    }

    /// Get id of WlCompositor globabl
    pub fn compositor_globabl(&self) -> GlobalId {
        self.compositor.clone()
    }

    /// Get id of WlSubcompositor globabl
    pub fn subcompositor_globabl(&self) -> GlobalId {
        self.subcompositor.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_attributes_empty() {
        let region = RegionAttributes { rects: vec![] };
        assert!(!region.contains((0, 0)));
    }

    #[test]
    fn region_attributes_add() {
        let region = RegionAttributes {
            rects: vec![(RectangleKind::Add, Rectangle::from_loc_and_size((0, 0), (10, 10)))],
        };

        assert!(region.contains((0, 0)));
    }

    #[test]
    fn region_attributes_add_subtract() {
        let region = RegionAttributes {
            rects: vec![
                (RectangleKind::Add, Rectangle::from_loc_and_size((0, 0), (10, 10))),
                (
                    RectangleKind::Subtract,
                    Rectangle::from_loc_and_size((0, 0), (5, 5)),
                ),
            ],
        };

        assert!(!region.contains((0, 0)));
        assert!(region.contains((5, 5)));
    }

    #[test]
    fn region_attributes_add_subtract_add() {
        let region = RegionAttributes {
            rects: vec![
                (RectangleKind::Add, Rectangle::from_loc_and_size((0, 0), (10, 10))),
                (
                    RectangleKind::Subtract,
                    Rectangle::from_loc_and_size((0, 0), (5, 5)),
                ),
                (RectangleKind::Add, Rectangle::from_loc_and_size((2, 2), (2, 2))),
            ],
        };

        assert!(!region.contains((0, 0)));
        assert!(region.contains((5, 5)));
        assert!(region.contains((2, 2)));
    }
}
