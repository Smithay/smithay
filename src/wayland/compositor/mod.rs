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
//! To initialize this implementation create the [`CompositorState`], store it inside your `State` struct
//! and implement the [`CompositorHandler`], as shown in this example:
//!
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! use smithay::delegate_compositor;
//! use smithay::wayland::compositor::{CompositorState, CompositorClientState, CompositorHandler};
//!
//! # struct State { compositor_state: CompositorState }
//! # struct ClientState { compositor_state: CompositorClientState }
//! # impl wayland_server::backend::ClientData for ClientState {}
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the compositor state
//! let compositor_state = CompositorState::new::<State>(
//!     &display.handle(),
//! );
//!
//! // insert the CompositorState into your state
//! // ..
//!
//! // implement the necessary traits
//! impl CompositorHandler for State {
//!    fn compositor_state(&mut self) -> &mut CompositorState {
//!        &mut self.compositor_state
//!    }
//!
//!    fn client_compositor_state<'a>(&self, client: &'a wayland_server::Client) -> &'a CompositorClientState {
//!        &client.get_data::<ClientState>().unwrap().compositor_state    
//!    }
//!
//!    fn commit(&mut self, surface: &wayland_server::protocol::wl_surface::WlSurface) {
//!        // called on every buffer commit.
//!        // .. your implementation ..
//!    }
//! }
//! delegate_compositor!(State);
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
//! 1. Pre Commit hooks registered to this surface are invoked. Such hooks can be registered using
//!    the [`add_pre_commit_hook`] function. They are typically used by protocol extensions that
//!    add state to a surface and need to check on commit that client did not request an
//!    illegal state before it is applied on commit.
//! 2. The pending state is either applied and made current, or cached for later application
//!    is the surface is a synchronize subsurface. If the current state is applied, state
//!    of the synchronized children subsurface are applied as well at this point.
//! 3. Post Commit hooks registered to this surface are invoked. Such hooks can be registered using
//!    the [`add_post_commit_hook`] function. They are typically used by abstractions that further process
//!    the state.
//! 4. Your implementation of [`CompositorHandler::commit`] is invoked, so that you can access
//!    the new current state of the surface. The state of sync children subsurfaces of your
//!    surface may have changed as well, so this is the place to check it, using functions
//!    like [`with_surface_tree_upward`] or [`with_surface_tree_downward`]. On the other hand,
//!    if the surface is a sync subsurface, its current state will note have changed as
//!    the result of that commit. You can check if it is using [`is_sync_subsurface`].
//! 5. If the surface is destroyed, destruction hooks are invoked. Such hooks can be registered
//!    using the [`add_destruction_hook`] function. They are typically used to cleanup associated
//!    state.
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

use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::{any::Any, sync::Mutex};

pub use self::cache::{Cacheable, CachedState, MultiCache};
pub use self::handlers::{RegionUserData, SubsurfaceCachedState, SubsurfaceUserData, SurfaceUserData};
use self::transaction::TransactionQueue;
pub use self::transaction::{Barrier, Blocker, BlockerState};
pub use self::tree::{AlreadyHasRole, TraversalAction};
use self::tree::{PrivateSurfaceData, SuggestedSurfaceState};
pub use crate::utils::hook::HookId;
use crate::utils::Transform;
use crate::utils::{user_data::UserDataMap, Buffer, Logical, Point, Rectangle};
use atomic_float::AtomicF64;
use wayland_server::backend::GlobalId;
use wayland_server::protocol::wl_compositor::WlCompositor;
use wayland_server::protocol::wl_subcompositor::WlSubcompositor;
use wayland_server::protocol::{wl_buffer, wl_callback, wl_output, wl_region, wl_surface::WlSurface};
use wayland_server::{Client, DisplayHandle, GlobalDispatch, Resource};

/// The role of a subsurface surface.
pub const SUBSURFACE_ROLE: &str = "subsurface";

/// Description of a part of a surface that
/// should be considered damaged and needs to be redrawn
#[derive(Debug, PartialEq, Eq)]
pub enum Damage {
    /// A rectangle containing the damaged zone, in surface coordinates
    Surface(Rectangle<i32, Logical>),
    /// A rectangle containing the damaged zone, in buffer coordinates
    ///
    /// Note: Buffer scaling must be taken into consideration
    Buffer(Rectangle<i32, Buffer>),
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
pub struct SurfaceData {
    /// The current role of the surface.
    ///
    /// If `None` if the surface has not yet been assigned a role
    pub role: Option<&'static str>,
    /// The non-buffered typemap storage of this surface
    pub data_map: UserDataMap,
    /// The double-buffered typemap storage of this surface
    pub cached_state: MultiCache,
}

/// New buffer assignation for a surface
#[derive(Debug)]
pub enum BufferAssignment {
    /// The surface no longer has a buffer attached to it
    Removed,
    /// A new buffer has been attached
    NewBuffer(wl_buffer::WlBuffer),
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

    /// Location of the new buffer relative to the previous one
    ///
    /// The x and y arguments specify the location of the new pending buffer's upper left corner,
    /// relative to the current buffer's upper left corner, in surface-local coordinates.
    ///
    /// In other words, the x and y, combined with the new surface size define in which directions
    /// the surface's size changes.
    ///
    /// You are free to set this field to `None` to avoid processing it several times.
    pub buffer_delta: Option<Point<i32, Logical>>,

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

    pub(crate) client_scale: f64,
}

impl Default for SurfaceAttributes {
    fn default() -> SurfaceAttributes {
        SurfaceAttributes {
            buffer: None,
            buffer_delta: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            opaque_region: None,
            input_region: None,
            damage: Vec::new(),
            frame_callbacks: Vec::new(),
            client_scale: 1.,
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
pub fn with_surface_tree_upward<F1, F2, F3, T>(
    surface: &WlSurface,
    initial: T,
    filter: F1,
    processor: F2,
    post_filter: F3,
) where
    F1: FnMut(&WlSurface, &SurfaceData, &T) -> TraversalAction<T>,
    F2: FnMut(&WlSurface, &SurfaceData, &T),
    F3: FnMut(&WlSurface, &SurfaceData, &T) -> bool,
{
    PrivateSurfaceData::map_tree(surface, &initial, filter, processor, post_filter, false);
}

/// Access the data of a surface tree from top to bottom
///
/// Behavior is the same as [`with_surface_tree_upward`], but the processing is done in the reverse order,
/// from the nearest of the screen to the deepest.
///
/// This would typically be used to find out which surface of a subsurface tree has been clicked for example.
pub fn with_surface_tree_downward<F1, F2, F3, T>(
    surface: &WlSurface,
    initial: T,
    filter: F1,
    processor: F2,
    post_filter: F3,
) where
    F1: FnMut(&WlSurface, &SurfaceData, &T) -> TraversalAction<T>,
    F2: FnMut(&WlSurface, &SurfaceData, &T),
    F3: FnMut(&WlSurface, &SurfaceData, &T) -> bool,
{
    PrivateSurfaceData::map_tree(surface, &initial, filter, processor, post_filter, true);
}

/// Retrieve the parent of this surface
///
/// Returns `None` is this surface is a root surface
pub fn get_parent(surface: &WlSurface) -> Option<WlSurface> {
    PrivateSurfaceData::get_parent(surface)
}

/// Retrieve the children of this surface
pub fn get_children(surface: &WlSurface) -> Vec<WlSurface> {
    PrivateSurfaceData::get_children(surface)
}

/// Check if this subsurface is a synchronized subsurface
pub fn is_sync_subsurface(surface: &WlSurface) -> bool {
    self::handlers::is_effectively_sync(surface)
}

/// Get the current role of this surface
pub fn get_role(surface: &WlSurface) -> Option<&'static str> {
    PrivateSurfaceData::get_role(surface)
}

/// Register that this surface has given role
///
/// Fails if the surface already has a role.
pub fn give_role(surface: &WlSurface, role: &'static str) -> Result<(), AlreadyHasRole> {
    PrivateSurfaceData::set_role(surface, role)
}

/// Access the states associated to this surface
pub fn with_states<F, T>(surface: &WlSurface, f: F) -> T
where
    F: FnOnce(&SurfaceData) -> T,
{
    PrivateSurfaceData::with_states(surface, f)
}

/// Send the `scale` and `transform` preferences for the given surface when it supports them.
///
/// The new state is only send when it differs from the already cached one on the calling thread.
pub fn send_surface_state(surface: &WlSurface, data: &SurfaceData, scale: i32, transform: Transform) {
    if surface.version() < 6 {
        return;
    }

    // NOTE we insert default for checks below to work properly.
    let mut storage = data
        .data_map
        .get_or_insert(|| RefCell::new(SuggestedSurfaceState::default()))
        .borrow_mut();

    if storage.scale != scale {
        surface.preferred_buffer_scale(scale);
        storage.scale = scale;
    }

    let transform = transform.into();
    if storage.transform != transform {
        surface.preferred_buffer_transform(transform);
        storage.transform = transform;
    }
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

/// Register a pre-commit hook to be invoked on surface commit
///
/// It'll be invoked on surface commit, *before* the new state is merged into the current state.
///
/// Protocol implementations should use this for error checking, but they should **not** apply
/// state changes here, since the commit may be further arbitrarily delayed by blockers. Use a
/// post-commit hook to apply state changes (i.e. copy last acked state to current).
///
/// Compositors should use this for adding blockers if needed, e.g. the DMA-BUF readiness blocker.
pub fn add_pre_commit_hook<D, F>(surface: &WlSurface, hook: F) -> HookId
where
    F: Fn(&mut D, &DisplayHandle, &WlSurface) + Send + Sync + 'static,
    D: 'static,
{
    let (user_state_type_id, user_state_type) = surface.data::<SurfaceUserData>().unwrap().user_state_type;
    assert_eq!(
        std::any::TypeId::of::<D>(),
        user_state_type_id,
        "D has to equal D used in CompositorState::new<D>(), {} != {}",
        std::any::type_name::<D>(),
        user_state_type,
    );

    let hook = move |state: &mut dyn Any, dh: &DisplayHandle, surface: &WlSurface| {
        let state = state.downcast_mut::<D>().unwrap();
        hook(state, dh, surface);
    };
    PrivateSurfaceData::add_pre_commit_hook(surface, hook)
}

/// Register a post-commit hook to be invoked on surface commit
///
/// It'll be invoked on surface commit, *after* the new state is merged into the current state,
/// after all commit blockers complete.
///
/// Protocol implementations should apply state changes here, i.e. copy last acked state into
/// current.
pub fn add_post_commit_hook<D, F>(surface: &WlSurface, hook: F) -> HookId
where
    F: Fn(&mut D, &DisplayHandle, &WlSurface) + Send + Sync + 'static,
    D: 'static,
{
    let (user_state_type_id, user_state_type) = surface.data::<SurfaceUserData>().unwrap().user_state_type;
    assert_eq!(
        std::any::TypeId::of::<D>(),
        user_state_type_id,
        "D has to equal D used in CompositorState::new<D>(), {} != {}",
        std::any::type_name::<D>(),
        user_state_type,
    );

    let hook = move |state: &mut dyn Any, dh: &DisplayHandle, surface: &WlSurface| {
        let state = state.downcast_mut::<D>().unwrap();
        hook(state, dh, surface);
    };
    PrivateSurfaceData::add_post_commit_hook(surface, hook)
}

/// Register a destruction hook to be invoked on surface destruction
///
/// It'll be invoked when the surface is destroyed (either explicitly by the client or on
/// client disconnect).
///
/// D generic is the compositor state, same as used in `CompositorState::new<D>()`
pub fn add_destruction_hook<D, F>(surface: &WlSurface, hook: F) -> HookId
where
    F: Fn(&mut D, &WlSurface) + Send + Sync + 'static,
    D: 'static,
{
    let (user_state_type_id, user_state_type) = surface.data::<SurfaceUserData>().unwrap().user_state_type;
    assert_eq!(
        std::any::TypeId::of::<D>(),
        user_state_type_id,
        "D has to equal D used in CompositorState::new<D>(), {} != {}",
        std::any::type_name::<D>(),
        user_state_type,
    );

    let hook = move |state: &mut dyn Any, surface: &WlSurface| {
        let state = state.downcast_mut::<D>().unwrap();
        hook(state, surface);
    };
    PrivateSurfaceData::add_destruction_hook(surface, hook)
}

/// Unregister a pre-commit hook
pub fn remove_pre_commit_hook(surface: &WlSurface, hook_id: HookId) {
    PrivateSurfaceData::remove_pre_commit_hook(surface, hook_id)
}

/// Unregister a post-commit hook
pub fn remove_post_commit_hook(surface: &WlSurface, hook_id: HookId) {
    PrivateSurfaceData::remove_post_commit_hook(surface, hook_id)
}

/// Unregister a destruction hook
pub fn remove_destruction_hook(surface: &WlSurface, hook_id: HookId) {
    PrivateSurfaceData::remove_destruction_hook(surface, hook_id)
}

/// Adds a blocker for the currently queued up state changes of the given surface.
///
/// Blockers will delay the pending state to be applied on the next commit until
/// all of them return the state `Released`. Any blocker returning `Cancelled` will
/// discard all changes.
///
/// The module will only evaluate blocker states on commit. If a blocker
/// becomes ready later, a call to [`CompositorClientState::blocker_cleared`] is necessary
/// to trigger a re-evaluation.
pub fn add_blocker(surface: &WlSurface, blocker: impl Blocker + Send + 'static) {
    PrivateSurfaceData::add_blocker(surface, blocker)
}

/// Handler trait for compositor
pub trait CompositorHandler {
    /// [CompositorState] getter
    fn compositor_state(&mut self) -> &mut CompositorState;
    /// [CompositorClientState] getter
    ///
    /// The compositor implementation needs some state to be client specific.
    /// Downstream is expected to store this inside its `ClientData` implementation(s)
    /// to ensure automatic cleanup of the state, when the client disconnects.
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState;

    /// New surface handler.
    ///
    /// This handler can be used to setup hooks (see [`add_pre_commit_hook`]/[`add_post_commit_hook`]/[`add_destruction_hook`]),
    /// but not much else. The surface has no role or attached data at this point and cannot be rendered.
    fn new_surface(&mut self, surface: &WlSurface) {
        let _ = surface;
    }

    /// New subsurface handler.
    ///
    /// This handler can be used to run extra logic when subsurface is getting created. This
    /// is an addition to [`new_surface`], which will be run for the subsurface surface anyway.
    ///
    /// When your compositor knows beforehand where it'll position subsurfaces it can send
    /// [`send_surface_state`] to them.
    ///
    /// [`new_surface`]: Self::new_surface
    fn new_subsurface(&mut self, surface: &WlSurface, parent: &WlSurface) {
        let _ = surface;
        let _ = parent;
    }

    /// Surface commit handler
    ///
    /// This is called when any changed state from a commit actually becomes visible.
    /// That might be some time after the actual commit has taken place, if the
    /// state changes are delayed by an added blocker (see [`add_blocker`]).
    ///
    /// If you need to handle a commit as soon as it occurs, you might want to consider
    /// using a pre-commit hook (see [`add_pre_commit_hook`]).
    fn commit(&mut self, surface: &WlSurface);

    /// The surface was destroyed.
    ///
    /// This allows the compositor to clean up any uses of the surface.
    fn destroyed(&mut self, _surface: &WlSurface) {}
}

/// State of a compositor
#[derive(Debug)]
pub struct CompositorState {
    compositor: GlobalId,
    subcompositor: GlobalId,
    surfaces: Vec<WlSurface>,
}

/// Per-client state of a compositor
#[derive(Debug)]
pub struct CompositorClientState {
    queue: Mutex<Option<TransactionQueue>>,
    scale_override: Arc<AtomicF64>,
}

impl Default for CompositorClientState {
    fn default() -> Self {
        CompositorClientState {
            queue: Mutex::new(None),
            scale_override: Arc::new(AtomicF64::new(1.)),
        }
    }
}

impl CompositorClientState {
    /// To be called, when a previously added blocker (via [`add_blocker`])
    /// got `Released` or `Cancelled` from being `Pending` previously for any
    /// surface belonging to this client.
    pub fn blocker_cleared<D: CompositorHandler + 'static>(&self, state: &mut D, dh: &DisplayHandle) {
        let transactions = if let Some(queue) = self.queue.lock().unwrap().as_mut() {
            queue.take_ready()
        } else {
            Vec::new()
        };

        for transaction in transactions {
            transaction.apply(dh, state)
        }
    }

    /// Set an additionally mapping between smithay's `Logical` coordinate space
    /// and this clients logical coordinate space.
    ///
    /// This is used in the same way as if the client was setting the
    /// surface.buffer_scale on every surface i.e a value of 2.0 will make
    /// the windows appear smaller on a regular DPI monitor.
    ///
    /// Only the minimal set of protocols used by xwayland are guaranteed to be supported.
    ///
    /// Buffer sizes are unaffected.
    pub fn set_client_scale(&self, new_scale: f64) {
        self.scale_override.store(new_scale, Ordering::Release);
    }

    /// Get the scale factor of the additional mapping between smithay's `Logical`
    /// coordinate space and this clients logical coordinate space.
    ///
    /// This is mainly intended to support out-of-tree protocol implementations.
    pub fn client_scale(&self) -> f64 {
        self.scale_override.load(Ordering::Acquire)
    }

    pub(crate) fn clone_client_scale(&self) -> Arc<AtomicF64> {
        self.scale_override.clone()
    }
}

impl CompositorState {
    /// Create new [`wl_compositor`] version 5 and [`wl_subcompositor`] globals.
    ///
    /// It returns the two global handles, in case you wish to remove these globals from
    /// the event loop in the future.
    ///
    /// [`wl_compositor`]: wayland_server::protocol::wl_compositor
    /// [`wl_subcompositor`]: wayland_server::protocol::wl_subcompositor
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<WlCompositor, ()> + GlobalDispatch<WlSubcompositor, ()> + 'static,
    {
        Self::new_with_version::<D>(display, 5)
    }

    /// The same as [`new`], but binds at least version 6 of [`wl_compositor`].
    ///
    /// This means that for clients to scale and apply transformation with
    /// non-default values [`send_surface_state`] must be used.
    ///
    /// [`new`]: Self::new
    /// [`wl_compositor`]: wayland_server::protocol::wl_compositor
    pub fn new_v6<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<WlCompositor, ()> + GlobalDispatch<WlSubcompositor, ()> + 'static,
    {
        Self::new_with_version::<D>(display, 6)
    }

    fn new_with_version<D>(display: &DisplayHandle, version: u32) -> Self
    where
        D: GlobalDispatch<WlCompositor, ()> + GlobalDispatch<WlSubcompositor, ()> + 'static,
    {
        let compositor = display.create_global::<D, WlCompositor, ()>(version, ());
        let subcompositor = display.create_global::<D, WlSubcompositor, ()>(1, ());

        CompositorState {
            compositor,
            subcompositor,
            surfaces: Vec::new(),
        }
    }

    /// Get id of compositor global
    pub fn compositor_global(&self) -> GlobalId {
        self.compositor.clone()
    }

    /// Get id of subcompositor global
    pub fn subcompositor_global(&self) -> GlobalId {
        self.subcompositor.clone()
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_compositor {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_compositor::WlCompositor: ()
        ] => $crate::wayland::compositor::CompositorState);
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_subcompositor::WlSubcompositor: ()
        ] => $crate::wayland::compositor::CompositorState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_compositor::WlCompositor: ()
        ] => $crate::wayland::compositor::CompositorState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_surface::WlSurface: $crate::wayland::compositor::SurfaceUserData
        ] => $crate::wayland::compositor::CompositorState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_region::WlRegion: $crate::wayland::compositor::RegionUserData
        ] => $crate::wayland::compositor::CompositorState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_callback::WlCallback: ()
        ] => $crate::wayland::compositor::CompositorState);
            // WlSubcompositor
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_subcompositor::WlSubcompositor: ()
        ] => $crate::wayland::compositor::CompositorState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_server::protocol::wl_subsurface::WlSubsurface: $crate::wayland::compositor::SubsurfaceUserData
        ] => $crate::wayland::compositor::CompositorState);
    };
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
            rects: vec![(RectangleKind::Add, Rectangle::from_size((10, 10).into()))],
        };

        assert!(region.contains((0, 0)));
    }

    #[test]
    fn region_attributes_add_subtract() {
        let region = RegionAttributes {
            rects: vec![
                (RectangleKind::Add, Rectangle::from_size((10, 10).into())),
                (RectangleKind::Subtract, Rectangle::from_size((5, 5).into())),
            ],
        };

        assert!(!region.contains((0, 0)));
        assert!(region.contains((5, 5)));
    }

    #[test]
    fn region_attributes_add_subtract_add() {
        let region = RegionAttributes {
            rects: vec![
                (RectangleKind::Add, Rectangle::from_size((10, 10).into())),
                (RectangleKind::Subtract, Rectangle::from_size((5, 5).into())),
                (RectangleKind::Add, Rectangle::new((2, 2).into(), (2, 2).into())),
            ],
        };

        assert!(!region.contains((0, 0)));
        assert!(region.contains((5, 5)));
        assert!(region.contains((2, 2)));
    }
}
