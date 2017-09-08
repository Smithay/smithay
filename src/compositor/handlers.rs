use super::{CompositorHandler, Damage, Handler as UserHandler, Rectangle, RectangleKind, Role, RoleType,
            SubsurfaceRole};
use super::region::RegionData;
use super::tree::{Location, SurfaceData};
use wayland_server::{Client, Destroy, EventLoopHandle, Liveness, Resource};
use wayland_server::protocol::{wl_buffer, wl_callback, wl_compositor, wl_output, wl_region,
                               wl_subcompositor, wl_subsurface, wl_surface};

struct CompositorDestructor<U, R> {
    _t: ::std::marker::PhantomData<U>,
    _r: ::std::marker::PhantomData<R>,
}

/*
 * wl_compositor
 */

impl<U, R, H> wl_compositor::Handler for CompositorHandler<U, R, H>
where
    U: Default + Send + 'static,
    R: Default + Send + 'static,
    H: UserHandler<U, R> + Send + 'static,
{
    fn create_surface(&mut self, evqh: &mut EventLoopHandle, _: &Client, _: &wl_compositor::WlCompositor,
                      id: wl_surface::WlSurface) {
        trace!(self.log, "New surface created.");
        unsafe { SurfaceData::<U, R>::init(&id) };
        evqh.register_with_destructor::<_, CompositorHandler<U, R, H>, CompositorDestructor<U, R>>(
            &id,
            self.my_id,
        );
    }
    fn create_region(&mut self, evqh: &mut EventLoopHandle, _: &Client, _: &wl_compositor::WlCompositor,
                     id: wl_region::WlRegion) {
        trace!(self.log, "New region created.");
        unsafe { RegionData::init(&id) };
        evqh.register_with_destructor::<_, CompositorHandler<U, R, H>, CompositorDestructor<U, R>>(
            &id,
            self.my_id,
        );
    }
}

server_declare_handler!(CompositorHandler<U: [Default, Send], R: [Default, Send], H: [UserHandler<U, R>, Send]>, wl_compositor::Handler, wl_compositor::WlCompositor);

/*
 * wl_surface
 */

impl<U, R, H: UserHandler<U, R>> wl_surface::Handler for CompositorHandler<U, R, H> {
    fn attach(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
              buffer: Option<&wl_buffer::WlBuffer>, x: i32, y: i32) {
        trace!(self.log, "Attaching buffer to surface.");
        unsafe {
            SurfaceData::<U, R>::with_data(surface, |d| {
                d.buffer = Some(buffer.map(|b| (b.clone_unchecked(), (x, y))))
            });
        }
    }
    fn damage(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface, x: i32,
              y: i32, width: i32, height: i32) {
        trace!(self.log, "Registering damage to surface.");
        unsafe {
            SurfaceData::<U, R>::with_data(surface, |d| {
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
            SurfaceData::<U, R>::with_data(surface, |d| d.opaque_region = attributes);
        }
    }
    fn set_input_region(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
                        region: Option<&wl_region::WlRegion>) {
        trace!(self.log, "Setting surface input region.");
        unsafe {
            let attributes = region.map(|r| RegionData::get_attributes(r));
            SurfaceData::<U, R>::with_data(surface, |d| d.input_region = attributes);
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
            SurfaceData::<U, R>::with_data(surface, |d| d.buffer_transform = transform);
        }
    }
    fn set_buffer_scale(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
                        scale: i32) {
        trace!(self.log, "Setting surface's buffer scale.");
        unsafe {
            SurfaceData::<U, R>::with_data(surface, |d| d.buffer_scale = scale);
        }
    }
    fn damage_buffer(&mut self, _: &mut EventLoopHandle, _: &Client, surface: &wl_surface::WlSurface,
                     x: i32, y: i32, width: i32, height: i32) {
        trace!(
            self.log,
            "Registering damage to surface (buffer coordinates)."
        );
        unsafe {
            SurfaceData::<U, R>::with_data(surface, |d| {
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

server_declare_handler!(CompositorHandler<U:[], R: [], H: [UserHandler<U, R>]>, wl_surface::Handler, wl_surface::WlSurface);

impl<U, R> Destroy<wl_surface::WlSurface> for CompositorDestructor<U, R> {
    fn destroy(surface: &wl_surface::WlSurface) {
        unsafe { SurfaceData::<U, R>::cleanup(surface) }
    }
}

/*
 * wl_region
 */

impl<U, R, H> wl_region::Handler for CompositorHandler<U, R, H> {
    fn add(&mut self, _: &mut EventLoopHandle, _: &Client, region: &wl_region::WlRegion, x: i32, y: i32,
           width: i32, height: i32) {
        trace!(self.log, "Adding rectangle to a region.");
        unsafe {
            RegionData::add_rectangle(
                region,
                RectangleKind::Add,
                Rectangle {
                    x,
                    y,
                    width,
                    height,
                },
            )
        };
    }
    fn subtract(&mut self, _: &mut EventLoopHandle, _: &Client, region: &wl_region::WlRegion, x: i32,
                y: i32, width: i32, height: i32) {
        trace!(self.log, "Subtracting rectangle to a region.");
        unsafe {
            RegionData::add_rectangle(
                region,
                RectangleKind::Subtract,
                Rectangle {
                    x,
                    y,
                    width,
                    height,
                },
            )
        };
    }
}

server_declare_handler!(CompositorHandler<U: [], R: [], H: []>, wl_region::Handler, wl_region::WlRegion);

impl<U, R> Destroy<wl_region::WlRegion> for CompositorDestructor<U, R> {
    fn destroy(region: &wl_region::WlRegion) {
        unsafe { RegionData::cleanup(region) };
    }
}

/*
 * wl_subcompositor
 */

impl<U, R, H> wl_subcompositor::Handler for CompositorHandler<U, R, H>
where
    U: Send + 'static,
    R: RoleType + Role<SubsurfaceRole> + Send + 'static,
    H: Send + 'static,
{
    fn get_subsurface(&mut self, evqh: &mut EventLoopHandle, _: &Client,
                      resource: &wl_subcompositor::WlSubcompositor, id: wl_subsurface::WlSubsurface,
                      surface: &wl_surface::WlSurface, parent: &wl_surface::WlSurface) {
        trace!(self.log, "Creating new subsurface.");
        if let Err(()) = unsafe { SurfaceData::<U, R>::set_parent(surface, parent) } {
            resource.post_error(
                wl_subcompositor::Error::BadSurface as u32,
                "Surface already has a role.".into(),
            );
            return;
        }
        id.set_user_data(
            Box::into_raw(Box::new(unsafe { surface.clone_unchecked() })) as *mut _,
        );
        evqh.register_with_destructor::<_, CompositorHandler<U, R, H>, CompositorDestructor<U, R>>(
            &id,
            self.my_id,
        );
    }
}

server_declare_handler!(CompositorHandler<U: [Send], R: [RoleType, Role<SubsurfaceRole>, Send], H: [Send]>, wl_subcompositor::Handler, wl_subcompositor::WlSubcompositor);

/*
 * wl_subsurface
 */

unsafe fn with_subsurface_attributes<U, R, F>(subsurface: &wl_subsurface::WlSubsurface, f: F)
where
    F: FnOnce(&mut SubsurfaceRole),
    R: RoleType + Role<SubsurfaceRole>,
{
    let ptr = subsurface.get_user_data();
    let surface = &*(ptr as *mut wl_surface::WlSurface);
    SurfaceData::<U, R>::with_role_data::<SubsurfaceRole, _, _>(surface, |d| f(d)).expect(
        "The surface does not have a subsurface role while it has a wl_subsurface?!",
    );
}

impl<U, R, H> wl_subsurface::Handler for CompositorHandler<U, R, H>
where
    R: RoleType + Role<SubsurfaceRole>,
{
    fn set_position(&mut self, _: &mut EventLoopHandle, _: &Client,
                    subsurface: &wl_subsurface::WlSubsurface, x: i32, y: i32) {
        trace!(self.log, "Setting subsurface position.");
        unsafe {
            with_subsurface_attributes::<U, R, _>(subsurface, |attrs| {
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
            if let Err(()) = SurfaceData::<U, R>::reorder(surface, Location::After, sibling) {
                subsurface.post_error(
                    wl_subsurface::Error::BadSurface as u32,
                    "Provided surface is not a sibling or parent.".into(),
                );
            }
        }
    }
    fn place_below(&mut self, _: &mut EventLoopHandle, _: &Client,
                   subsurface: &wl_subsurface::WlSubsurface, sibling: &wl_surface::WlSurface) {
        trace!(self.log, "Setting subsurface below an other.");
        unsafe {
            let ptr = subsurface.get_user_data();
            let surface = &*(ptr as *mut wl_surface::WlSurface);
            if let Err(()) = SurfaceData::<U, R>::reorder(surface, Location::Before, sibling) {
                subsurface.post_error(
                    wl_subsurface::Error::BadSurface as u32,
                    "Provided surface is not a sibling or parent.".into(),
                );
            }
        }
    }
    fn set_sync(&mut self, _: &mut EventLoopHandle, _: &Client, subsurface: &wl_subsurface::WlSubsurface) {
        trace!(self.log, "Setting subsurface sync."; "sync_status" => true);
        unsafe {
            with_subsurface_attributes::<U, R, _>(subsurface, |attrs| { attrs.sync = true; });
        }
    }
    fn set_desync(&mut self, _: &mut EventLoopHandle, _: &Client, subsurface: &wl_subsurface::WlSubsurface) {
        trace!(self.log, "Setting subsurface sync."; "sync_status" => false);
        unsafe {
            with_subsurface_attributes::<U, R, _>(subsurface, |attrs| { attrs.sync = false; });
        }
    }
}

server_declare_handler!(CompositorHandler<U: [], R: [RoleType, Role<SubsurfaceRole>], H: []>, wl_subsurface::Handler, wl_subsurface::WlSubsurface);

impl<U, R> Destroy<wl_subsurface::WlSubsurface> for CompositorDestructor<U, R>
where
    R: RoleType + Role<SubsurfaceRole>,
{
    fn destroy(subsurface: &wl_subsurface::WlSubsurface) {
        let ptr = subsurface.get_user_data();
        subsurface.set_user_data(::std::ptr::null_mut());
        unsafe {
            let surface = Box::from_raw(ptr as *mut wl_surface::WlSurface);
            if surface.status() == Liveness::Alive {
                SurfaceData::<U, R>::unset_parent(&surface);
            }
        }
    }
}
