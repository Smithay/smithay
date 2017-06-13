use super::{CompositorHandler, Damage, Handler as UserHandler, Rectangle, RectangleKind,
            SubsurfaceAttributes};
use super::region::RegionData;
use super::tree::{Location, SurfaceData};
use wayland_server::{Client, Destroy, EventLoopHandle, Liveness, Resource};
use wayland_server::protocol::{wl_buffer, wl_callback, wl_compositor, wl_output, wl_region,
                               wl_subcompositor, wl_subsurface, wl_surface};

struct CompositorDestructor<U> {
    _t: ::std::marker::PhantomData<U>,
}

/*
 * wl_compositor
 */

impl<U, H> wl_compositor::Handler for CompositorHandler<U, H>
    where U: Default + Send + 'static,
          H: UserHandler<U> + Send + 'static
{
    fn create_surface(&mut self, evqh: &mut EventLoopHandle, _: &Client, _: &wl_compositor::WlCompositor,
                      id: wl_surface::WlSurface) {
        trace!(self.log, "New surface created.");
        unsafe { SurfaceData::<U>::init(&id) };
        evqh.register_with_destructor::<_, CompositorHandler<U, H>, CompositorDestructor<U>>(&id, self.my_id);
    }
    fn create_region(&mut self, evqh: &mut EventLoopHandle, _: &Client, _: &wl_compositor::WlCompositor,
                     id: wl_region::WlRegion) {
        trace!(self.log, "New region created.");
        unsafe { RegionData::init(&id) };
        evqh.register_with_destructor::<_, CompositorHandler<U, H>, CompositorDestructor<U>>(&id, self.my_id);
    }
}

unsafe impl<U, H> ::wayland_server::Handler<wl_compositor::WlCompositor> for CompositorHandler<U, H>
    where U: Default + Send + 'static,
          H: UserHandler<U> + Send + 'static
{
    unsafe fn message(&mut self, evq: &mut EventLoopHandle, client: &Client,
                      resource: &wl_compositor::WlCompositor, opcode: u32,
                      args: *const ::wayland_server::sys::wl_argument)
                      -> Result<(), ()> {
        <CompositorHandler<U, H> as ::wayland_server::protocol::wl_compositor::Handler>::__message(self, evq, client, resource, opcode, args)
    }
}

/*
 * wl_surface
 */

impl<U, H: UserHandler<U>> wl_surface::Handler for CompositorHandler<U, H> {
    fn attach(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
              buffer: Option<&wl_buffer::WlBuffer>, x: i32, y: i32) {
        trace!(self.log, "Attaching buffer to surface.");
        unsafe {
            SurfaceData::<U>::with_data(surface, |d| {
                d.buffer = Some(buffer.map(|b| (b.clone_unchecked(), (x, y))))
            });
        }
    }
    fn damage(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface, x: i32,
              y: i32, width: i32, height: i32) {
        trace!(self.log, "Registering damage to surface.");
        unsafe {
            SurfaceData::<U>::with_data(surface, |d| {
                d.damage = Damage::Surface(Rectangle {
                                               x,
                                               y,
                                               width,
                                               height,
                                           })
            });
        }
    }
    fn frame(&mut self, evlh: &mut EventLoopHandle, client: &Client, surface: &wl_surface::WlSurface,
             callback: wl_callback::WlCallback) {
        trace!(self.log, "Frame surface callback.");
        let token = self.get_token();
        UserHandler::frame(&mut self.handler, evlh, client, surface, callback, token);
    }
    fn set_opaque_region(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
                         region: Option<&wl_region::WlRegion>) {
        trace!(self.log, "Setting surface opaque region.");
        unsafe {
            let attributes = region.map(|r| RegionData::get_attributes(r));
            SurfaceData::<U>::with_data(surface, |d| d.opaque_region = attributes);
        }
    }
    fn set_input_region(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
                        region: Option<&wl_region::WlRegion>) {
        trace!(self.log, "Setting surface input region.");
        unsafe {
            let attributes = region.map(|r| RegionData::get_attributes(r));
            SurfaceData::<U>::with_data(surface, |d| d.input_region = attributes);
        }
    }
    fn commit(&mut self, evlh: &mut EventLoopHandle, client: &Client, surface: &wl_surface::WlSurface) {
        trace!(self.log, "Commit surface callback.");
        let token = self.get_token();
        UserHandler::commit(&mut self.handler, evlh, client, surface, token);
    }
    fn set_buffer_transform(&mut self, _: &mut EventLoopHandle, _: &Client,
                            surface: &wl_surface::WlSurface, transform: wl_output::Transform) {
        trace!(self.log, "Setting surface's buffer transform.");
        unsafe {
            SurfaceData::<U>::with_data(surface, |d| d.buffer_transform = transform);
        }
    }
    fn set_buffer_scale(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
                        scale: i32) {
        trace!(self.log, "Setting surface's buffer scale.");
        unsafe {
            SurfaceData::<U>::with_data(surface, |d| d.buffer_scale = scale);
        }
    }
    fn damage_buffer(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
                     x: i32, y: i32, width: i32, height: i32) {
        trace!(self.log,
               "Registering damage to surface (buffer coordinates).");
        unsafe {
            SurfaceData::<U>::with_data(surface, |d| {
                d.damage = Damage::Buffer(Rectangle {
                                              x,
                                              y,
                                              width,
                                              height,
                                          })
            });
        }
    }
}

unsafe impl<U, H: UserHandler<U>> ::wayland_server::Handler<wl_surface::WlSurface>
    for CompositorHandler<U, H> {
    unsafe fn message(&mut self, evq: &mut EventLoopHandle, client: &Client,
                      resource: &wl_surface::WlSurface, opcode: u32,
                      args: *const ::wayland_server::sys::wl_argument)
                      -> Result<(), ()> {
        <CompositorHandler<U, H> as ::wayland_server::protocol::wl_surface::Handler>::__message(self, evq, client, resource, opcode, args)
    }
}

impl<U> Destroy<wl_surface::WlSurface> for CompositorDestructor<U> {
    fn destroy(surface: &wl_surface::WlSurface) {
        unsafe { SurfaceData::<U>::cleanup(surface) }
    }
}

/*
 * wl_region
 */

impl<U, H> wl_region::Handler for CompositorHandler<U, H> {
    fn add(&mut self, _: &mut EventLoopHandle, _: &Client, region: &wl_region::WlRegion, x: i32, y: i32,
           width: i32, height: i32) {
        trace!(self.log, "Adding rectangle to a region.");
        unsafe {
            RegionData::add_rectangle(region,
                                      RectangleKind::Add,
                                      Rectangle {
                                          x,
                                          y,
                                          width,
                                          height,
                                      })
        };
    }
    fn subtract(&mut self, _: &mut EventLoopHandle, _: &Client, region: &wl_region::WlRegion, x: i32,
                y: i32, width: i32, height: i32) {
        trace!(self.log, "Subtracting rectangle to a region.");
        unsafe {
            RegionData::add_rectangle(region,
                                      RectangleKind::Subtract,
                                      Rectangle {
                                          x,
                                          y,
                                          width,
                                          height,
                                      })
        };
    }
}

unsafe impl<U, H> ::wayland_server::Handler<wl_region::WlRegion> for CompositorHandler<U, H> {
    unsafe fn message(&mut self, evq: &mut EventLoopHandle, client: &Client,
                      resource: &wl_region::WlRegion, opcode: u32,
                      args: *const ::wayland_server::sys::wl_argument)
                      -> Result<(), ()> {
        <CompositorHandler<U, H> as ::wayland_server::protocol::wl_region::Handler>::__message(self, evq, client, resource, opcode, args)
    }
}

impl<U> Destroy<wl_region::WlRegion> for CompositorDestructor<U> {
    fn destroy(region: &wl_region::WlRegion) {
        unsafe { RegionData::cleanup(region) };
    }
}

/*
 * wl_subcompositor
 */

impl<U, H> wl_subcompositor::Handler for CompositorHandler<U, H>
    where U: Send + 'static,
          H: Send + 'static
{
    fn get_subsurface(&mut self, evqh: &mut EventLoopHandle, _: &Client,
                      resource: &wl_subcompositor::WlSubcompositor, id: wl_subsurface::WlSubsurface,
                      surface: &wl_surface::WlSurface, parent: &wl_surface::WlSurface) {
        trace!(self.log, "Creating new subsurface.");
        if let Err(()) = unsafe { SurfaceData::<U>::set_parent(surface, parent) } {
            resource.post_error(wl_subcompositor::Error::BadSurface as u32, "Surface already has a role.".into());
            return
        }
        id.set_user_data(Box::into_raw(Box::new(unsafe { surface.clone_unchecked() })) as *mut _);
        unsafe {
            SurfaceData::<U>::with_data(surface, |d| {
                d.subsurface_attributes = Some(Default::default())
            });
        }
        evqh.register_with_destructor::<_, CompositorHandler<U, H>, CompositorDestructor<U>>(&id, self.my_id);
    }
}

unsafe impl<U, H> ::wayland_server::Handler<wl_subcompositor::WlSubcompositor> for CompositorHandler<U, H>
    where U: Send + 'static,
          H: Send + 'static
{
    unsafe fn message(&mut self, evq: &mut EventLoopHandle, client: &Client,
                      resource: &wl_subcompositor::WlSubcompositor, opcode: u32,
                      args: *const ::wayland_server::sys::wl_argument)
                      -> Result<(), ()> {
        <CompositorHandler<U, H> as ::wayland_server::protocol::wl_subcompositor::Handler>::__message(self, evq, client, resource, opcode, args)
    }
}

/*
 * wl_subsurface
 */

unsafe fn with_subsurface_attributes<U, F>(subsurface: &wl_subsurface::WlSubsurface, f: F)
    where F: FnOnce(&mut SubsurfaceAttributes)
{
    let ptr = subsurface.get_user_data();
    let surface = &*(ptr as *mut wl_surface::WlSurface);
    SurfaceData::<U>::with_data(surface, |d| f(d.subsurface_attributes.as_mut().unwrap()));
}

impl<U, H> wl_subsurface::Handler for CompositorHandler<U, H> {
    fn set_position(&mut self, _: &mut EventLoopHandle, _: &Client,
                    subsurface: &wl_subsurface::WlSubsurface, x: i32, y: i32) {
        trace!(self.log, "Setting subsurface position.");
        unsafe {
            with_subsurface_attributes::<U, _>(subsurface, |attrs| {
                attrs.x = x;
                attrs.y = y;
            });
        }
    }
    fn place_above(&mut self, _: &mut EventLoopHandle, _: &Client,
                   subsurface: &wl_subsurface::WlSubsurface, sibling: &wl_surface::WlSurface) {
        trace!(self.log, "Setting subsurface above an other.");
        unsafe {
            let ptr = subsurface.get_user_data();
            let surface = &*(ptr as *mut wl_surface::WlSurface);
            if let Err(()) = SurfaceData::<U>::reorder(surface, Location::After, sibling) {
                subsurface.post_error(wl_subsurface::Error::BadSurface as u32, "Provided surface is not a sibling or parent.".into());
            }
        }
    }
    fn place_below(&mut self, _: &mut EventLoopHandle, _: &Client,
                   subsurface: &wl_subsurface::WlSubsurface, sibling: &wl_surface::WlSurface) {
        trace!(self.log, "Setting subsurface below an other.");
        unsafe {
            let ptr = subsurface.get_user_data();
            let surface = &*(ptr as *mut wl_surface::WlSurface);
            if let Err(()) = SurfaceData::<U>::reorder(surface, Location::Before, sibling) {
                subsurface.post_error(wl_subsurface::Error::BadSurface as u32, "Provided surface is not a sibling or parent.".into());
            }
        }
    }
    fn set_sync(&mut self, _: &mut EventLoopHandle, _: &Client, subsurface: &wl_subsurface::WlSubsurface) {
        trace!(self.log, "Setting subsurface sync."; "sync_status" => true);
        unsafe {
            with_subsurface_attributes::<U, _>(subsurface, |attrs| { attrs.sync = true; });
        }
    }
    fn set_desync(&mut self, _: &mut EventLoopHandle, _: &Client, subsurface: &wl_subsurface::WlSubsurface) {
        trace!(self.log, "Setting subsurface sync."; "sync_status" => false);
        unsafe {
            with_subsurface_attributes::<U, _>(subsurface, |attrs| { attrs.sync = false; });
        }
    }
}

unsafe impl<U, H> ::wayland_server::Handler<wl_subsurface::WlSubsurface> for CompositorHandler<U, H> {
    unsafe fn message(&mut self, evq: &mut EventLoopHandle, client: &Client,
                      resource: &wl_subsurface::WlSubsurface, opcode: u32,
                      args: *const ::wayland_server::sys::wl_argument)
                      -> Result<(), ()> {
        <CompositorHandler<U, H> as ::wayland_server::protocol::wl_subsurface::Handler>::__message(self, evq, client, resource, opcode, args)
    }
}

impl<U> Destroy<wl_subsurface::WlSubsurface> for CompositorDestructor<U> {
    fn destroy(subsurface: &wl_subsurface::WlSubsurface) {
        let ptr = subsurface.get_user_data();
        subsurface.set_user_data(::std::ptr::null_mut());
        unsafe {
            let surface = Box::from_raw(ptr as *mut wl_surface::WlSurface);
            if surface.status() == Liveness::Alive {
                SurfaceData::<U>::with_data(&*surface, |d| d.subsurface_attributes = None);
                SurfaceData::<U>::unset_parent(&surface);
            }
        }
    }
}
