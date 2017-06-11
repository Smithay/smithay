mod global;
mod handlers;
mod tree;
mod region;

pub use self::tree::RoleStatus;
use self::tree::SurfaceData;
use wayland_server::{Client, EventLoopHandle, Init, resource_is_registered};

use wayland_server::protocol::{wl_buffer, wl_callback, wl_output, wl_surface};

/// Description of which part of a surface
/// should be considered damaged and needs to be redrawn
pub enum Damage {
    /// The whole surface must be considered damaged (this is the default)
    Full,
    /// A rectangle containing the damaged zone, in surface coordinates
    Surface(Rectangle),
    /// A rectangle containing the smaazed zone, in buffer coordinates
    ///
    /// Note: Buffer scaling must be taken into consideration
    Buffer(Rectangle),
}

/// Data associated with a surface, aggreged by the handlers
///
/// Most of the fields of this struct represent a double-buffered state, which
/// should only be applied once a `commit` request is received from the surface.
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
    /// times. It'll be set to `Some(...)` if the user attaches a buffer (or NULL) to
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
    /// Subsurface-related attribute
    ///
    /// Is `Some` if this surface is a sub-surface
    ///
    /// **Warning:** Changing this field by yourself can cause panics.
    pub subsurface_attributes: Option<SubsurfaceAttributes>,
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
            subsurface_attributes: None,
            user_data: Default::default(),
        }
    }
}

/// Attributes defining the behaviour of a sub-surface relative to its parent
pub struct SubsurfaceAttributes {
    /// Horizontal location of the top-left corner of this sub-surface relative to
    /// the top-left corner of its parent
    pub x: i32,
    /// Vertical location of the top-left corner of this sub-surface relative to
    /// the top-left corner of its parent
    pub y: i32,
    /// Sync status of this sub-surface
    ///
    /// If `true`, this surface should be repainted synchronously with its parent
    /// if `false`, it should be considered independant of its parent regarding
    /// repaint timings.
    pub sync: bool,
}

impl Default for SubsurfaceAttributes {
    fn default() -> SubsurfaceAttributes {
        SubsurfaceAttributes {
            x: 0,
            y: 0,
            sync: true,
        }
    }
}

/// Kind of a rectangle part of a region
#[derive(Copy,Clone)]
pub enum RectangleKind {
    /// This rectangle should be added to the region
    Add,
    /// The intersection of this rectangle with the region should
    /// be removed from the region
    Subtract,
}

/// A rectangle defined by its top-left corner and dimensions
#[derive(Copy,Clone)]
pub struct Rectangle {
    /// horizontal position of the top-leftcorner of the rectangle, in surface coordinates
    pub x: i32,
    /// vertical position of the top-leftcorner of the rectangle, in surface coordinates
    pub y: i32,
    /// width of the rectangle
    pub width: i32,
    /// height of the rectangle
    pub height: i32,
}

/// Description of the contents of a region
///
/// A region is defined as an union and difference of rectangle.
///
/// This struct contains an ordered Vec containing the rectangles defining
/// a region. They should be added or substracted in this order to compute the
/// actual contents of the region.
#[derive(Clone)]
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
/// access data associated with the wl_surface and wl_region managed
/// by the `CompositorGlobal` that provided it.
#[derive(Copy,Clone)]
pub struct CompositorToken<U, H> {
    hid: usize,
    _data: ::std::marker::PhantomData<*mut U>,
    _handler: ::std::marker::PhantomData<*mut H>,
}

impl<U: Send + 'static, H: Handler + Send + 'static> CompositorToken<U, H> {
    /// Access the data of a surface
    ///
    /// The closure will be called with the contents of the data associated with this surface.
    ///
    /// If the surface is not managed by the CompositorGlobal that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn with_surface_data<F>(&self, surface: &wl_surface::WlSurface, f: F)
        where F: FnOnce(&mut SurfaceAttributes<U>)
    {
        assert!(resource_is_registered::<_, CompositorHandler<U, H>>(surface, self.hid),
                "Accessing the data of foreign surfaces is not supported.");
        unsafe {
            SurfaceData::<U>::with_data(surface, f);
        }
    }

    /// Access the data of a surface tree
    ///
    /// The provided closure is called successively on the surface and all its child subsurfaces,
    /// in a depth-first order. This matches the order in which the surfaces are supposed to be
    /// drawn: top-most last.
    ///
    /// If the surface is not managed by the CompositorGlobal that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn with_surface_tree<F>(&self, surface: &wl_surface::WlSurface, f: F) -> Result<(), ()>
        where F: FnMut(&wl_surface::WlSurface, &mut SurfaceAttributes<U>) -> bool
    {
        assert!(resource_is_registered::<_, CompositorHandler<U, H>>(surface, self.hid),
                "Accessing the data of foreign surfaces is not supported.");
        unsafe {
            SurfaceData::<U>::map_tree(surface, f);
        }
        Ok(())
    }

    /// Retrieve the parent of this surface
    ///
    /// Returns `None` is this surface is a root surface
    ///
    /// If the surface is not managed by the CompositorGlobal that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn get_parent(&self, surface: &wl_surface::WlSurface) -> Option<wl_surface::WlSurface> {
        assert!(resource_is_registered::<_, CompositorHandler<U, H>>(surface, self.hid),
                "Accessing the data of foreign surfaces is not supported.");
        unsafe { SurfaceData::<U>::get_parent(surface) }
    }

    /// Retrieve the children of this surface
    ///
    /// If the surface is not managed by the CompositorGlobal that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn get_children(&self, surface: &wl_surface::WlSurface) -> Vec<wl_surface::WlSurface> {
        assert!(resource_is_registered::<_, CompositorHandler<U, H>>(surface, self.hid),
                "Accessing the data of foreign surfaces is not supported.");
        unsafe { SurfaceData::<U>::get_children(surface) }
    }

    /// Retrieve the role status this surface
    ///
    /// If the surface is not managed by the CompositorGlobal that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn role_status(&self, surface: &wl_surface::WlSurface) -> RoleStatus {
        assert!(resource_is_registered::<_, CompositorHandler<U, H>>(surface, self.hid),
                "Accessing the data of foreign surfaces is not supported.");
        unsafe { SurfaceData::<U>::role_status(surface) }
    }

    /// Register that this surface has a role
    ///
    /// This makes this surface impossible to become a subsurface, as
    /// a surface can only have a single role at a time.
    ///
    /// Fails if the surface already has a role.
    ///
    /// If the surface is not managed by the CompositorGlobal that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn give_role(&self, surface: &wl_surface::WlSurface) -> Result<(), ()> {
        assert!(resource_is_registered::<_, CompositorHandler<U, H>>(surface, self.hid),
                "Accessing the data of foreign surfaces is not supported.");
        unsafe { SurfaceData::<U>::give_role(surface) }
    }

    /// Register that this surface has no role
    ///
    /// It is a noop if this surface already didn't have one, but fails if
    /// the role was "subsurface". This role is automatically managed and as such
    /// cannot be removed manually.
    ///
    /// If the surface is not managed by the CompositorGlobal that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn remove_role(&self, surface: &wl_surface::WlSurface) -> Result<(), ()> {
        assert!(resource_is_registered::<_, CompositorHandler<U, H>>(surface, self.hid),
                "Accessing the data of foreign surfaces is not supported.");
        unsafe { SurfaceData::<U>::remove_role(surface) }
    }

    /// Retrieve the metadata associated with a wl_region
    ///
    /// If the region is not managed by the CompositorGlobal that provided this token, this
    /// will panic (having more than one compositor is not supported).
    pub fn get_region_attributes(&self, region: &wl_region::WlRegion) -> RegionAttributes {
        assert!(resource_is_registered::<_, CompositorHandler<U, H>>(region, self.hid),
                "Accessing the data of foreign regions is not supported.");
        unsafe { RegionData::get_attributes(region) }
    }
}

pub struct CompositorHandler<U, H> {
    my_id: usize,
    log: ::slog::Logger,
    handler: H,
    _data: ::std::marker::PhantomData<U>,
}

impl<U, H> Init for CompositorHandler<U, H> {
    fn init(&mut self, _evqh: &mut EventLoopHandle, index: usize) {
        self.my_id = index;
        debug!(self.log, "Init finished")
    }
}

impl<U, H> CompositorHandler<U, H> {
    /// Create a new CompositorHandler
    pub fn new<L>(handler: H, logger: L) -> CompositorHandler<U, H>
        where L: Into<Option<::slog::Logger>>
    {
        let log = ::slog_or_stdlog(logger);
        CompositorHandler {
            my_id: ::std::usize::MAX,
            log: log.new(o!("smithay_module" => "compositor_handler")),
            handler: handler,
            _data: ::std::marker::PhantomData,
        }
    }

    /// Create a token to access the data associated to the objects managed by this handler.
    pub fn get_token(&self) -> CompositorToken<U, H> {
        assert!(self.my_id != ::std::usize::MAX,
                "CompositorHandler is not initialized yet.");
        CompositorToken {
            hid: self.my_id,
            _data: ::std::marker::PhantomData,
            _handler: ::std::marker::PhantomData,
        }
    }

    /// Access the underlying sub-handler
    pub fn get_handler(&mut self) -> &mut H {
        &mut self.handler
    }
}

/// Sub-handler trait for surface event handling
///
/// The global provided by Smithay cannot process these events for you, so they
/// are forwarded directly to a handler implementing this trait that you must provide
/// at creation of the `CompositorHandler`.
#[allow(unused_variables)]
pub trait Handler {
    /// See `wayland_server::protocol::wl_surface::Handler::commit`
    fn commit(&mut self, evlh: &mut EventLoopHandle, client: &Client, surface: &wl_surface::WlSurface) {}
    /// See `wayland_server::protocol::wl_surface::Handler::frame`
    fn frame(&mut self, evlh: &mut EventLoopHandle, client: &Client, surface: &wl_surface::WlSurface,
             callback: wl_callback::WlCallback) {
    }
}
