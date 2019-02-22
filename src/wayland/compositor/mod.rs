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
//! surface trees with subsurfaces, to provide you a direct access to the tree
//! structure and all surface metadata.
//!
//! As such, you can, given a root surface with a role requiring it to be displayed,
//! you can iterate over the whole tree of subsurfaces to recover all the metadata you
//! need to display the subsurface tree.
//!
//! This implementation will not do anything more than present you the metadata specified by the
//! client in a coherent and practical way. All the logic regarding to drawing itself, and
//! the positioning of windows (surface trees) one relative to another is out of its scope.
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this implementation, use the [`compositor_init`](::wayland::compositor::compositor_init)
//! method provided by this module. It'll require you to first define as few things, as shown in
//! this example:
//!
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! use smithay::wayland::compositor::compositor_init;
//!
//! // Define some user data to be associated with the surfaces.
//! // It must implement the Default trait, which will represent the state of a surface which
//! // has just been created.
//! #[derive(Default)]
//! struct MyData {
//!     // whatever you need here
//! }
//!
//! // Declare the roles enum
//! define_roles!(MyRoles);
//!
//! # fn main() {
//! # let mut event_loop = wayland_server::calloop::EventLoop::<()>::new().unwrap();
//! # let mut display = wayland_server::Display::new(event_loop.handle());
//! // Call the init function:
//! let (token, _, _) = compositor_init::<MyData, MyRoles, _, _>(
//!     &mut display,
//!     |request, surface, compositor_token| {
//!         /*
//!           Your handling of the user requests.
//!          */
//!     },
//!     None // put a logger here
//! );
//!
//! // this `token` is what you'll use to access the surface associated data
//!
//! // You're now ready to go!
//! # }
//! ```
//!
//! ### Use the surface metadata
//!
//! As you can see in the previous example, in the end we are retrieving a token from
//! the `init` function. This token is necessary to retrieve the metadata associated with
//! a surface. It can be cloned. See [`CompositorToken`](::wayland::compositor::CompositorToken)
//! for the details of what it enables you.
//!
//! The surface metadata is held in the [`SurfaceAttributes`](::wayland::compositor::SurfaceAttributes)
//! struct. In contains double-buffered state pending from the client as defined by the protocol for
//! [`wl_surface`](wayland_server::protocol::wl_surface), as well as your user-defined type holding
//! any data you need to have associated with a struct. See its documentation for details.
//!
//! This [`CompositorToken`](::wayland::compositor::CompositorToken) also provides access to the metadata associated with the role of the
//! surfaces. See the documentation of the [`roles`](::wayland::compositor::roles) submodule
//! for a detailed explanation.

use std::{cell::RefCell, rc::Rc, sync::Mutex};

mod handlers;
pub mod roles;
mod tree;

pub use self::tree::TraversalAction;
use self::{
    roles::{Role, RoleType, WrongRole},
    tree::SurfaceData,
};
use crate::utils::Rectangle;
use wayland_server::{
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_output, wl_region, wl_subcompositor, wl_surface::WlSurface,
    },
    Display, Global, NewResource,
};

/// Description of which part of a surface
/// should be considered damaged and needs to be redrawn
pub enum Damage {
    /// The whole surface must be considered damaged (this is the default)
    Full,
    /// A rectangle containing the damaged zone, in surface coordinates
    Surface(Rectangle),
    /// A rectangle containing the damaged zone, in buffer coordinates
    ///
    /// Note: Buffer scaling must be taken into consideration
    Buffer(Rectangle),
}

#[derive(Copy, Clone, Default)]
struct Marker<U, R> {
    _u: ::std::marker::PhantomData<U>,
    _r: ::std::marker::PhantomData<R>,
}

/// Data associated with a surface, aggregated by the handlers
///
/// Most of the fields of this struct represent a double-buffered state, which
/// should only be applied once a [`commit`](::wayland::compositor::SurfaceEvent::Commit)
/// request is received from the surface.
///
/// You are responsible for setting those values as you see fit to avoid
/// processing them two times.
pub struct SurfaceAttributes<U> {
    /// Buffer defining the contents of the surface
    ///
    /// The tuple represent the coordinates of this buffer
    /// relative to the location of the current buffer.
    ///
    /// If set to `Some(None)`, it means the user specifically asked for the
    /// surface to be unmapped.
    ///
    /// You are free to set this field to `None` to avoid processing it several
    /// times. It'll be set to `Some(...)` if the user attaches a buffer (or `NULL`) to
    /// the surface.
    pub buffer: Option<Option<(wl_buffer::WlBuffer, (i32, i32))>>,
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
    pub damage: Damage,
    /// User-controlled data
    ///
    /// This is your field to host whatever you need.
    pub user_data: U,
}

impl<U: Default> Default for SurfaceAttributes<U> {
    fn default() -> SurfaceAttributes<U> {
        SurfaceAttributes {
            buffer: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            opaque_region: None,
            input_region: None,
            damage: Damage::Full,
            user_data: Default::default(),
        }
    }
}

/// Attributes defining the behaviour of a sub-surface relative to its parent
#[derive(Copy, Clone, Debug)]
pub struct SubsurfaceRole {
    /// Location of the top-left corner of this sub-surface relative to
    /// the top-left corner of its parent
    pub location: (i32, i32),
    /// Sync status of this sub-surface
    ///
    /// If `true`, this surface should be repainted synchronously with its parent
    /// if `false`, it should be considered independent of its parent regarding
    /// repaint timings.
    pub sync: bool,
}

impl Default for SubsurfaceRole {
    fn default() -> SubsurfaceRole {
        SubsurfaceRole {
            location: (0, 0),
            sync: true,
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
#[derive(Clone, Debug)]
pub struct RegionAttributes {
    /// List of rectangle part of this region
    pub rects: Vec<(RectangleKind, Rectangle)>,
}

impl Default for RegionAttributes {
    fn default() -> RegionAttributes {
        RegionAttributes { rects: Vec::new() }
    }
}

/// A Compositor global token
///
/// This token can be cloned at will, and is the entry-point to
/// access data associated with the [`wl_surface`](wayland_server::protocol::wl_surface)
/// and [`wl_region`](wayland_server::protocol::wl_region) managed
/// by the `CompositorGlobal` that provided it.
pub struct CompositorToken<U, R> {
    _data: ::std::marker::PhantomData<*mut U>,
    _role: ::std::marker::PhantomData<*mut R>,
}

// we implement them manually because #[derive(..)] would require
// U: Clone and R: Clone
impl<U, R> Copy for CompositorToken<U, R> {}
impl<U, R> Clone for CompositorToken<U, R> {
    fn clone(&self) -> CompositorToken<U, R> {
        *self
    }
}

unsafe impl<U, R> Send for CompositorToken<U, R> {}
unsafe impl<U, R> Sync for CompositorToken<U, R> {}

impl<U, R> CompositorToken<U, R> {
    pub(crate) fn make() -> CompositorToken<U, R> {
        CompositorToken {
            _data: ::std::marker::PhantomData,
            _role: ::std::marker::PhantomData,
        }
    }
}

impl<U: 'static, R: 'static> CompositorToken<U, R> {
    /// Access the data of a surface
    ///
    /// The closure will be called with the contents of the data associated with this surface.
    ///
    /// If the surface is not managed by the `CompositorGlobal` that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn with_surface_data<F, T>(&self, surface: &WlSurface, f: F) -> T
    where
        F: FnOnce(&mut SurfaceAttributes<U>) -> T,
    {
        SurfaceData::<U, R>::with_data(surface, f)
    }
}

impl<U, R> CompositorToken<U, R>
where
    U: 'static,
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    /// Access the data of a surface tree from bottom to top
    ///
    /// The provided closure is called successively on the surface and all its child subsurfaces,
    /// in a depth-first order. This matches the order in which the surfaces are supposed to be
    /// drawn: top-most last.
    ///
    /// The arguments provided to the closure are, in this order:
    ///
    /// - The surface object itself
    /// - a mutable reference to its surface attribute data
    /// - a mutable reference to its role data,
    /// - a custom value that is passed in a fold-like manner, but only from the output of a parent
    ///   to its children. See [`TraversalAction`] for details.
    ///
    /// If the surface not managed by the `CompositorGlobal` that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn with_surface_tree_upward<F, T>(&self, surface: &WlSurface, initial: T, f: F) -> Result<(), ()>
    where
        F: FnMut(&WlSurface, &mut SurfaceAttributes<U>, &mut R, &T) -> TraversalAction<T>,
    {
        SurfaceData::<U, R>::map_tree(surface, initial, f, false);
        Ok(())
    }

    /// Access the data of a surface tree from top to bottom
    ///
    /// The provided closure is called successively on the surface and all its child subsurfaces,
    /// in a depth-first order. This matches the reverse of the order in which the surfaces are
    /// supposed to be drawn: top-most first.
    ///
    /// Behavior is the same as [`with_surface_tree_upward`](CompositorToken::with_surface_tree_upward).
    pub fn with_surface_tree_downward<F, T>(&self, surface: &WlSurface, initial: T, f: F) -> Result<(), ()>
    where
        F: FnMut(&WlSurface, &mut SurfaceAttributes<U>, &mut R, &T) -> TraversalAction<T>,
    {
        SurfaceData::<U, R>::map_tree(surface, initial, f, true);
        Ok(())
    }

    /// Retrieve the parent of this surface
    ///
    /// Returns `None` is this surface is a root surface
    ///
    /// If the surface is not managed by the `CompositorGlobal` that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn get_parent(&self, surface: &WlSurface) -> Option<WlSurface> {
        SurfaceData::<U, R>::get_parent(surface)
    }

    /// Retrieve the children of this surface
    ///
    /// If the surface is not managed by the `CompositorGlobal` that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn get_children(&self, surface: &WlSurface) -> Vec<WlSurface> {
        SurfaceData::<U, R>::get_children(surface)
    }
}

impl<U: 'static, R: RoleType + 'static> CompositorToken<U, R> {
    /// Check whether this surface as a role or not
    ///
    /// If the surface is not managed by the `CompositorGlobal` that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn has_a_role(&self, surface: &WlSurface) -> bool {
        SurfaceData::<U, R>::has_a_role(surface)
    }

    /// Check whether this surface as a specific role
    ///
    /// If the surface is not managed by the `CompositorGlobal` that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn has_role<RoleData>(&self, surface: &WlSurface) -> bool
    where
        R: Role<RoleData>,
    {
        SurfaceData::<U, R>::has_role::<RoleData>(surface)
    }

    /// Register that this surface has given role with default data
    ///
    /// Fails if the surface already has a role.
    ///
    /// If the surface is not managed by the `CompositorGlobal` that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn give_role<RoleData>(&self, surface: &WlSurface) -> Result<(), ()>
    where
        R: Role<RoleData>,
        RoleData: Default,
    {
        SurfaceData::<U, R>::give_role::<RoleData>(surface)
    }

    /// Register that this surface has given role with given data
    ///
    /// Fails if the surface already has a role and returns the data.
    ///
    /// If the surface is not managed by the `CompositorGlobal` that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn give_role_with<RoleData>(&self, surface: &WlSurface, data: RoleData) -> Result<(), RoleData>
    where
        R: Role<RoleData>,
    {
        SurfaceData::<U, R>::give_role_with::<RoleData>(surface, data)
    }

    /// Access the role data of a surface
    ///
    /// Fails and don't call the closure if the surface doesn't have this role
    ///
    /// If the surface is not managed by the `CompositorGlobal` that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn with_role_data<RoleData, F, T>(&self, surface: &WlSurface, f: F) -> Result<T, WrongRole>
    where
        R: Role<RoleData>,
        F: FnOnce(&mut RoleData) -> T,
    {
        SurfaceData::<U, R>::with_role_data::<RoleData, _, _>(surface, f)
    }

    /// Register that this surface does not have a role any longer and retrieve the data
    ///
    /// Fails if the surface didn't already have this role.
    ///
    /// If the surface is not managed by the `CompositorGlobal` that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn remove_role<RoleData>(&self, surface: &WlSurface) -> Result<RoleData, WrongRole>
    where
        R: Role<RoleData>,
    {
        SurfaceData::<U, R>::remove_role::<RoleData>(surface)
    }

    /// Retrieve the metadata associated with a `wl_region`
    ///
    /// If the region is not managed by the `CompositorGlobal` that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn get_region_attributes(&self, region: &wl_region::WlRegion) -> RegionAttributes {
        match region.as_ref().user_data::<Mutex<RegionAttributes>>() {
            Some(mutex) => mutex.lock().unwrap().clone(),
            None => panic!("Accessing the data of foreign regions is not supported."),
        }
    }
}

/// Create new [`wl_compositor`](wayland_server::protocol::wl_compositor)
/// and [`wl_subcompositor`](wayland_server::protocol::wl_subcompositor) globals.
///
/// The globals are directly registered into the event loop, and this function
/// returns a [`CompositorToken`] which you'll need access the data associated to
/// the [`wl_surface`](wayland_server::protocol::wl_surface)s.
///
/// It also returns the two global handles, in case you wish to remove these
/// globals from the event loop in the future.
pub fn compositor_init<U, R, Impl, L>(
    display: &mut Display,
    implem: Impl,
    logger: L,
) -> (
    CompositorToken<U, R>,
    Global<wl_compositor::WlCompositor>,
    Global<wl_subcompositor::WlSubcompositor>,
)
where
    L: Into<Option<::slog::Logger>>,
    U: Default + 'static,
    R: Default + RoleType + Role<SubsurfaceRole> + 'static,
    Impl: FnMut(SurfaceEvent, WlSurface, CompositorToken<U, R>) + 'static,
{
    let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "compositor_handler"));
    let implem = Rc::new(RefCell::new(implem));

    let compositor = display.create_global(4, move |new_compositor, _version| {
        self::handlers::implement_compositor::<U, R, Impl>(new_compositor, log.clone(), implem.clone());
    });

    let subcompositor = display.create_global(1, move |new_subcompositor, _version| {
        self::handlers::implement_subcompositor::<U, R>(new_subcompositor);
    });

    (CompositorToken::make(), compositor, subcompositor)
}

/// User-handled events for surfaces
///
/// The global provided by smithay cannot process these events for you, so
/// they are forwarded directly via your provided implementation, and are
/// described by this global.
pub enum SurfaceEvent {
    /// The double-buffered state has been validated by the client
    ///
    /// At this point, the pending state that has been accumulated in the [`SurfaceAttributes`] associated
    /// to this surface should be integrated into the current state of the surface.
    ///
    /// See [`wayland_server::protocol::wl_surface::Implementation::commit`](https://docs.rs/wayland-server/0.10.1/wayland_server/protocol/wl_surface/struct.Implementation.html#structfield.commit)
    /// for more details
    Commit,
    /// The client asks to be notified when would be a good time to update the contents of this surface
    ///
    /// You must keep the provided [`WlCallback`](wayland_server::protocol::wl_callback::WlCallback)
    /// and trigger it at the appropriate time by calling its `done()` method.
    ///
    /// See [`wayland_server::protocol::wl_surface::Implementation::frame`](https://docs.rs/wayland-server/0.10.1/wayland_server/protocol/wl_surface/struct.Implementation.html#structfield.frame)
    /// for more details
    Frame {
        /// The created `WlCallback`
        callback: NewResource<wl_callback::WlCallback>,
    },
}
