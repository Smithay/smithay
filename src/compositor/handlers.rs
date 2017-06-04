use super::{Rectangle, RectangleKind, SubsurfaceAttributes, Damage};
use super::region::RegionData;
use super::tree::SurfaceData;
use wayland_server::{Client, Destroy, EventLoopHandle, Init, Resource};
use wayland_server::protocol::{wl_buffer, wl_callback, wl_compositor, wl_output, wl_region,
                               wl_subcompositor, wl_subsurface, wl_surface};

pub struct CompositorHandler<U> {
    my_id: usize,
    log: ::slog::Logger,
    _data: ::std::marker::PhantomData<U>,
}

struct CompositorDestructor<U> {
    _t: ::std::marker::PhantomData<U>,
}

impl<U> Init for CompositorHandler<U> {
    fn init(&mut self, _evqh: &mut EventLoopHandle, index: usize) {
        self.my_id = index;
        debug!(self.log, "Init finished")
    }
}

impl<U> CompositorHandler<U> {
    pub fn new(log: ::slog::Logger) -> CompositorHandler<U> {
        CompositorHandler {
            my_id: ::std::usize::MAX,
            log: log,
            _data: ::std::marker::PhantomData::<U>,
        }
    }
}

/*
 * wl_compositor
 */

impl<U: Default + Send + 'static> wl_compositor::Handler for CompositorHandler<U> {
    fn create_surface(&mut self, evqh: &mut EventLoopHandle, _: &Client,
                      _: &wl_compositor::WlCompositor, id: wl_surface::WlSurface) {
        unsafe { SurfaceData::<U>::init(&id) };
        evqh.register_with_destructor::<_, CompositorHandler<U>, CompositorDestructor<U>>(&id, self.my_id);
    }
    fn create_region(&mut self, evqh: &mut EventLoopHandle, _: &Client,
                     _: &wl_compositor::WlCompositor, id: wl_region::WlRegion) {
        unsafe { RegionData::init(&id) };
        evqh.register_with_destructor::<_, CompositorHandler<U>, CompositorDestructor<U>>(&id, self.my_id);
    }
}

unsafe impl<U: Default + Send + 'static> ::wayland_server::Handler<wl_compositor::WlCompositor>
    for CompositorHandler<U> {
    unsafe fn message(&mut self, evq: &mut EventLoopHandle, client: &Client,
                      resource: &wl_compositor::WlCompositor, opcode: u32,
                      args: *const ::wayland_server::sys::wl_argument)
                      -> Result<(), ()> {
        <CompositorHandler<U> as ::wayland_server::protocol::wl_compositor::Handler>::__message(self, evq, client, resource, opcode, args)
    }
}

/*
 * wl_surface
 */

impl<U> wl_surface::Handler for CompositorHandler<U> {
    fn attach(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
              buffer: Option<&wl_buffer::WlBuffer>, x: i32, y: i32) {
        unsafe {
            SurfaceData::<U>::with_data(surface,
                                        |d| d.buffer = Some(buffer.map(|b| (b.clone_unchecked(), (x, y)))));
        }
    }
    fn damage(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface, x: i32,
              y: i32, width: i32, height: i32) {
        unsafe {
            SurfaceData::<U>::with_data(surface,
                                        |d| d.damage = Damage::Surface(Rectangle { x, y, width, height }));
        }
    }
    fn frame(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
             callback: wl_callback::WlCallback) {
        unimplemented!()
    }
    fn set_opaque_region(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
                         region: Option<&wl_region::WlRegion>) {
        unsafe {
            let attributes = region.map(|r| RegionData::get_attributes(r));
            SurfaceData::<U>::with_data(surface, |d| d.opaque_region = attributes);
        }
    }
    fn set_input_region(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
                        region: Option<&wl_region::WlRegion>) {
        unsafe {
            let attributes = region.map(|r| RegionData::get_attributes(r));
            SurfaceData::<U>::with_data(surface, |d| d.input_region = attributes);
        }
    }
    fn commit(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface) {
        unimplemented!()
    }
    fn set_buffer_transform(&mut self, _: &mut EventLoopHandle, _: &Client,
                            surface: &wl_surface::WlSurface, transform: wl_output::Transform) {
        unsafe {
            SurfaceData::<U>::with_data(surface, |d| d.buffer_transform = transform);
        }
    }
    fn set_buffer_scale(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
                        scale: i32) {
        unsafe {
            SurfaceData::<U>::with_data(surface, |d| d.buffer_scale = scale);
        }
    }
    fn damage_buffer(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
                     x: i32, y: i32, width: i32, height: i32) {
        unsafe {
            SurfaceData::<U>::with_data(surface,
                                        |d| d.damage = Damage::Buffer(Rectangle { x, y, width, height }));
        }
    }
}

unsafe impl<U> ::wayland_server::Handler<wl_surface::WlSurface> for CompositorHandler<U> {
    unsafe fn message(&mut self, evq: &mut EventLoopHandle, client: &Client,
                      resource: &wl_surface::WlSurface, opcode: u32,
                      args: *const ::wayland_server::sys::wl_argument)
                      -> Result<(), ()> {
        <CompositorHandler<U> as ::wayland_server::protocol::wl_surface::Handler>::__message(self, evq, client, resource, opcode, args)
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

impl<U> wl_region::Handler for CompositorHandler<U> {
    fn add(&mut self, _: &mut EventLoopHandle, _: &Client, region: &wl_region::WlRegion, x: i32, y: i32,
           width: i32, height: i32) {
        unsafe {
            RegionData::add_rectangle(region, RectangleKind::Add,
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
        unsafe {
            RegionData::add_rectangle(region, RectangleKind::Subtract,
                                      Rectangle {
                                          x,
                                          y,
                                          width,
                                          height,
                                      })
        };
    }
}

unsafe impl<U> ::wayland_server::Handler<wl_region::WlRegion> for CompositorHandler<U> {
    unsafe fn message(&mut self, evq: &mut EventLoopHandle, client: &Client,
                      resource: &wl_region::WlRegion, opcode: u32,
                      args: *const ::wayland_server::sys::wl_argument)
                      -> Result<(), ()> {
        <CompositorHandler<U> as ::wayland_server::protocol::wl_region::Handler>::__message(self, evq, client, resource, opcode, args)
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

impl<U: Send + 'static> wl_subcompositor::Handler for CompositorHandler<U> {
    fn get_subsurface(&mut self, evqh: &mut EventLoopHandle, _: &Client,
                      resource: &wl_subcompositor::WlSubcompositor, id: wl_subsurface::WlSubsurface,
                      surface: &wl_surface::WlSurface, parent: &wl_surface::WlSurface) {
        if let Err(()) = unsafe { SurfaceData::<U>::set_parent(surface, parent) } {
            resource.post_error(wl_subcompositor::Error::BadSurface as u32, "Surface already has a role.".into());
            return
        }
        id.set_user_data(Box::into_raw(Box::new(unsafe { surface.clone_unchecked() })) as *mut _);
        unsafe {
            SurfaceData::<U>::with_data(surface,
                                        |d| d.subsurface_attributes = Some(Default::default()));
        }
        evqh.register_with_destructor::<_, CompositorHandler<U>, CompositorDestructor<U>>(&id, self.my_id);
    }
}

unsafe impl<U: Send + 'static> ::wayland_server::Handler<wl_subcompositor::WlSubcompositor>
    for CompositorHandler<U> {
    unsafe fn message(&mut self, evq: &mut EventLoopHandle, client: &Client,
                      resource: &wl_subcompositor::WlSubcompositor, opcode: u32,
                      args: *const ::wayland_server::sys::wl_argument)
                      -> Result<(), ()> {
        <CompositorHandler<U> as ::wayland_server::protocol::wl_subcompositor::Handler>::__message(self, evq, client, resource, opcode, args)
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

impl<U> wl_subsurface::Handler for CompositorHandler<U> {
    fn set_position(&mut self, _: &mut EventLoopHandle, _: &Client,
                    resource: &wl_subsurface::WlSubsurface, x: i32, y: i32) {
        unsafe {
            with_subsurface_attributes::<U, _>(resource, |attrs| {
                attrs.x = x;
                attrs.y = y;
            });
        }
    }
    fn place_above(&mut self, _: &mut EventLoopHandle, _: &Client,
                   resource: &wl_subsurface::WlSubsurface, sibling: &wl_surface::WlSurface) {
        unimplemented!()
    }
    fn place_below(&mut self, _: &mut EventLoopHandle, _: &Client,
                   resource: &wl_subsurface::WlSubsurface, sibling: &wl_surface::WlSurface) {
        unimplemented!()
    }
    fn set_sync(&mut self, _: &mut EventLoopHandle, _: &Client,
                resource: &wl_subsurface::WlSubsurface) {
        unsafe {
            with_subsurface_attributes::<U, _>(resource, |attrs| { attrs.sync = true; });
        }
    }
    fn set_desync(&mut self, _: &mut EventLoopHandle, _: &Client,
                  resource: &wl_subsurface::WlSubsurface) {
        unsafe {
            with_subsurface_attributes::<U, _>(resource, |attrs| { attrs.sync = false; });
        }
    }
}

unsafe impl<U> ::wayland_server::Handler<wl_subsurface::WlSubsurface> for CompositorHandler<U> {
    unsafe fn message(&mut self, evq: &mut EventLoopHandle, client: &Client,
                      resource: &wl_subsurface::WlSubsurface, opcode: u32,
                      args: *const ::wayland_server::sys::wl_argument)
                      -> Result<(), ()> {
        <CompositorHandler<U> as ::wayland_server::protocol::wl_subsurface::Handler>::__message(self, evq, client, resource, opcode, args)
    }
}

impl<U> Destroy<wl_subsurface::WlSubsurface> for CompositorDestructor<U> {
    fn destroy(subsurface: &wl_subsurface::WlSubsurface) {
        let ptr = subsurface.get_user_data();
        subsurface.set_user_data(::std::ptr::null_mut());
        unsafe {
            let surface = Box::from_raw(ptr as *mut wl_surface::WlSurface);
            SurfaceData::<U>::with_data(&*surface, |d| d.subsurface_attributes = None);
            SurfaceData::<U>::unset_parent(&surface);
        }
    }
}
