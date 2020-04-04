use std::{cell::RefCell, ops::Deref as _, rc::Rc, sync::Mutex};

use wayland_server::{
    protocol::{wl_compositor, wl_region, wl_subcompositor, wl_subsurface, wl_surface},
    Filter, Main,
};

use super::{
    tree::{Location, SurfaceData},
    CompositorToken, Damage, Rectangle, RectangleKind, RegionAttributes, Role, RoleType, SubsurfaceRole,
    SurfaceEvent,
};

/*
 * wl_compositor
 */

pub(crate) fn implement_compositor<R, Impl>(
    compositor: Main<wl_compositor::WlCompositor>,
    log: ::slog::Logger,
    implem: Rc<RefCell<Impl>>,
) -> wl_compositor::WlCompositor
where
    R: Default + Send + 'static,
    Impl: FnMut(SurfaceEvent, wl_surface::WlSurface, CompositorToken<R>) + 'static,
{
    compositor.quick_assign(move |_compositor, request, _| match request {
        wl_compositor::Request::CreateSurface { id } => {
            trace!(log, "Creating a new wl_surface.");
            implement_surface(id, log.clone(), implem.clone());
        }
        wl_compositor::Request::CreateRegion { id } => {
            trace!(log, "Creating a new wl_region.");
            implement_region(id);
        }
        _ => unreachable!(),
    });
    compositor.deref().clone()
}

/*
 * wl_surface
 */

// Internal implementation data of surfaces
pub(crate) struct SurfaceImplem<R> {
    log: ::slog::Logger,
    implem: Rc<RefCell<dyn FnMut(SurfaceEvent, wl_surface::WlSurface, CompositorToken<R>)>>,
}

impl<R> SurfaceImplem<R> {
    fn make<Impl>(log: ::slog::Logger, implem: Rc<RefCell<Impl>>) -> SurfaceImplem<R>
    where
        Impl: FnMut(SurfaceEvent, wl_surface::WlSurface, CompositorToken<R>) + 'static,
    {
        SurfaceImplem { log, implem }
    }
}

impl<R> SurfaceImplem<R>
where
    R: 'static,
{
    fn receive_surface_request(&mut self, req: wl_surface::Request, surface: wl_surface::WlSurface) {
        match req {
            wl_surface::Request::Attach { buffer, x, y } => {
                SurfaceData::<R>::with_data(&surface, |d| d.buffer = Some(buffer.map(|b| (b, (x, y)))));
            }
            wl_surface::Request::Damage { x, y, width, height } => {
                SurfaceData::<R>::with_data(&surface, |d| {
                    d.damage = Damage::Surface(Rectangle { x, y, width, height })
                });
            }
            wl_surface::Request::Frame { callback } => {
                let mut user_impl = self.implem.borrow_mut();
                trace!(self.log, "Calling user implementation for wl_surface.frame");
                (&mut *user_impl)(SurfaceEvent::Frame { callback }, surface, CompositorToken::make());
            }
            wl_surface::Request::SetOpaqueRegion { region } => {
                let attributes = region.map(|r| {
                    let attributes_mutex = r.as_ref().user_data().get::<Mutex<RegionAttributes>>().unwrap();
                    attributes_mutex.lock().unwrap().clone()
                });
                SurfaceData::<R>::with_data(&surface, |d| d.opaque_region = attributes);
            }
            wl_surface::Request::SetInputRegion { region } => {
                let attributes = region.map(|r| {
                    let attributes_mutex = r.as_ref().user_data().get::<Mutex<RegionAttributes>>().unwrap();
                    attributes_mutex.lock().unwrap().clone()
                });
                SurfaceData::<R>::with_data(&surface, |d| d.input_region = attributes);
            }
            wl_surface::Request::Commit => {
                let mut user_impl = self.implem.borrow_mut();
                trace!(self.log, "Calling user implementation for wl_surface.commit");
                (&mut *user_impl)(SurfaceEvent::Commit, surface, CompositorToken::make());
            }
            wl_surface::Request::SetBufferTransform { transform } => {
                SurfaceData::<R>::with_data(&surface, |d| d.buffer_transform = transform);
            }
            wl_surface::Request::SetBufferScale { scale } => {
                SurfaceData::<R>::with_data(&surface, |d| d.buffer_scale = scale);
            }
            wl_surface::Request::DamageBuffer { x, y, width, height } => {
                SurfaceData::<R>::with_data(&surface, |d| {
                    d.damage = Damage::Buffer(Rectangle { x, y, width, height })
                });
            }
            wl_surface::Request::Destroy => {
                // All is already handled by our destructor
            }
            _ => unreachable!(),
        }
    }
}

fn implement_surface<R, Impl>(
    surface: Main<wl_surface::WlSurface>,
    log: ::slog::Logger,
    implem: Rc<RefCell<Impl>>,
) -> wl_surface::WlSurface
where
    R: Default + Send + 'static,
    Impl: FnMut(SurfaceEvent, wl_surface::WlSurface, CompositorToken<R>) + 'static,
{
    surface.quick_assign({
        let mut implem = SurfaceImplem::make(log, implem);
        move |surface, req, _| implem.receive_surface_request(req, surface.deref().clone())
    });
    surface.assign_destructor(Filter::new(|surface, _, _| SurfaceData::<R>::cleanup(&surface)));
    surface
        .as_ref()
        .user_data()
        .set_threadsafe(|| SurfaceData::<R>::new());
    SurfaceData::<R>::init(&surface);
    surface.deref().clone()
}

/*
 * wl_region
 */

fn region_implem(request: wl_region::Request, region: wl_region::WlRegion) {
    let attributes_mutex = region
        .as_ref()
        .user_data()
        .get::<Mutex<RegionAttributes>>()
        .unwrap();
    let mut guard = attributes_mutex.lock().unwrap();
    match request {
        wl_region::Request::Add { x, y, width, height } => guard
            .rects
            .push((RectangleKind::Add, Rectangle { x, y, width, height })),
        wl_region::Request::Subtract { x, y, width, height } => guard
            .rects
            .push((RectangleKind::Subtract, Rectangle { x, y, width, height })),
        wl_region::Request::Destroy => {
            // all is handled by our destructor
        }
        _ => unreachable!(),
    }
}

fn implement_region(region: Main<wl_region::WlRegion>) -> wl_region::WlRegion {
    region.quick_assign(|region, req, _| region_implem(req, region.deref().clone()));
    region
        .as_ref()
        .user_data()
        .set_threadsafe(|| Mutex::new(RegionAttributes::default()));
    region.deref().clone()
}

/*
 * wl_subcompositor
 */

pub(crate) fn implement_subcompositor<R>(
    subcompositor: Main<wl_subcompositor::WlSubcompositor>,
) -> wl_subcompositor::WlSubcompositor
where
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    subcompositor.quick_assign(move |subcompositor, request, _| match request {
        wl_subcompositor::Request::GetSubsurface { id, surface, parent } => {
            if let Err(()) = SurfaceData::<R>::set_parent(&surface, &parent) {
                subcompositor.as_ref().post_error(
                    wl_subcompositor::Error::BadSurface as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }
            implement_subsurface::<R>(id, surface);
        }
        wl_subcompositor::Request::Destroy => {}
        _ => unreachable!(),
    });
    subcompositor.deref().clone()
}

/*
 * wl_subsurface
 */

fn with_subsurface_attributes<R, F>(subsurface: &wl_subsurface::WlSubsurface, f: F)
where
    F: FnOnce(&mut SubsurfaceRole),
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    let surface = subsurface
        .as_ref()
        .user_data()
        .get::<wl_surface::WlSurface>()
        .unwrap();
    SurfaceData::<R>::with_role_data::<SubsurfaceRole, _, _>(surface, |d| f(d))
        .expect("The surface does not have a subsurface role while it has a wl_subsurface?!");
}

fn implement_subsurface<R>(
    subsurface: Main<wl_subsurface::WlSubsurface>,
    surface: wl_surface::WlSurface,
) -> wl_subsurface::WlSubsurface
where
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    subsurface.quick_assign(|subsurface, request, _| {
        match request {
            wl_subsurface::Request::SetPosition { x, y } => {
                with_subsurface_attributes::<R, _>(&subsurface, |attrs| {
                    attrs.location = (x, y);
                })
            }
            wl_subsurface::Request::PlaceAbove { sibling } => {
                let surface = subsurface
                    .as_ref()
                    .user_data()
                    .get::<wl_surface::WlSurface>()
                    .unwrap();
                if let Err(()) = SurfaceData::<R>::reorder(surface, Location::After, &sibling) {
                    subsurface.as_ref().post_error(
                        wl_subsurface::Error::BadSurface as u32,
                        "Provided surface is not a sibling or parent.".into(),
                    )
                }
            }
            wl_subsurface::Request::PlaceBelow { sibling } => {
                let surface = subsurface
                    .as_ref()
                    .user_data()
                    .get::<wl_surface::WlSurface>()
                    .unwrap();
                if let Err(()) = SurfaceData::<R>::reorder(surface, Location::Before, &sibling) {
                    subsurface.as_ref().post_error(
                        wl_subsurface::Error::BadSurface as u32,
                        "Provided surface is not a sibling or parent.".into(),
                    )
                }
            }
            wl_subsurface::Request::SetSync => with_subsurface_attributes::<R, _>(&subsurface, |attrs| {
                attrs.sync = true;
            }),
            wl_subsurface::Request::SetDesync => with_subsurface_attributes::<R, _>(&subsurface, |attrs| {
                attrs.sync = false;
            }),
            wl_subsurface::Request::Destroy => {
                // Our destructor already handles it
            }
            _ => unreachable!(),
        }
    });
    subsurface.assign_destructor(Filter::new(|subsurface, _, _| {
        destroy_subsurface::<R>(&subsurface)
    }));
    subsurface.as_ref().user_data().set_threadsafe(|| surface);
    subsurface.deref().clone()
}

fn destroy_subsurface<R>(subsurface: &wl_subsurface::WlSubsurface)
where
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    let surface = subsurface
        .as_ref()
        .user_data()
        .get::<wl_surface::WlSurface>()
        .unwrap();
    if surface.as_ref().is_alive() {
        SurfaceData::<R>::unset_parent(&surface);
    }
}
