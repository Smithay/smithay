use std::{cell::RefCell, rc::Rc, sync::Mutex};

use wayland_server::{
    protocol::{wl_compositor, wl_region, wl_subcompositor, wl_subsurface, wl_surface},
    NewResource,
};

use super::{
    tree::{Location, SurfaceData},
    CompositorToken, Damage, Rectangle, RectangleKind, RegionAttributes, Role, RoleType, SubsurfaceRole,
    SurfaceEvent,
};

/*
 * wl_compositor
 */

pub(crate) fn implement_compositor<U, R, Impl>(
    compositor: NewResource<wl_compositor::WlCompositor>,
    log: ::slog::Logger,
    implem: Rc<RefCell<Impl>>,
) -> wl_compositor::WlCompositor
where
    U: Default + 'static,
    R: Default + 'static,
    Impl: FnMut(SurfaceEvent, wl_surface::WlSurface, CompositorToken<U, R>) + 'static,
{
    compositor.implement_closure(
        move |request, _compositor| match request {
            wl_compositor::Request::CreateSurface { id } => {
                trace!(log, "Creating a new wl_surface.");
                implement_surface(id, log.clone(), implem.clone());
            }
            wl_compositor::Request::CreateRegion { id } => {
                trace!(log, "Creating a new wl_region.");
                implement_region(id);
            }
            _ => unreachable!(),
        },
        None::<fn(_)>,
        (),
    )
}

/*
 * wl_surface
 */

// Internal implementation data of surfaces
pub(crate) struct SurfaceImplem<U, R> {
    log: ::slog::Logger,
    implem: Rc<RefCell<dyn FnMut(SurfaceEvent, wl_surface::WlSurface, CompositorToken<U, R>)>>,
}

impl<U, R> SurfaceImplem<U, R> {
    fn make<Impl>(log: ::slog::Logger, implem: Rc<RefCell<Impl>>) -> SurfaceImplem<U, R>
    where
        Impl: FnMut(SurfaceEvent, wl_surface::WlSurface, CompositorToken<U, R>) + 'static,
    {
        SurfaceImplem { log, implem }
    }
}

impl<U, R> SurfaceImplem<U, R>
where
    U: 'static,
    R: 'static,
{
    fn receive_surface_request(&mut self, req: wl_surface::Request, surface: wl_surface::WlSurface) {
        match req {
            wl_surface::Request::Attach { buffer, x, y } => {
                SurfaceData::<U, R>::with_data(&surface, |d| {
                    d.buffer = Some(buffer.map(|b| (b.clone(), (x, y))))
                });
            }
            wl_surface::Request::Damage { x, y, width, height } => {
                SurfaceData::<U, R>::with_data(&surface, |d| {
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
                    let attributes_mutex = r.as_ref().user_data::<Mutex<RegionAttributes>>().unwrap();
                    attributes_mutex.lock().unwrap().clone()
                });
                SurfaceData::<U, R>::with_data(&surface, |d| d.opaque_region = attributes);
            }
            wl_surface::Request::SetInputRegion { region } => {
                let attributes = region.map(|r| {
                    let attributes_mutex = r.as_ref().user_data::<Mutex<RegionAttributes>>().unwrap();
                    attributes_mutex.lock().unwrap().clone()
                });
                SurfaceData::<U, R>::with_data(&surface, |d| d.input_region = attributes);
            }
            wl_surface::Request::Commit => {
                let mut user_impl = self.implem.borrow_mut();
                trace!(self.log, "Calling user implementation for wl_surface.commit");
                (&mut *user_impl)(SurfaceEvent::Commit, surface, CompositorToken::make());
            }
            wl_surface::Request::SetBufferTransform { transform } => {
                SurfaceData::<U, R>::with_data(&surface, |d| d.buffer_transform = transform);
            }
            wl_surface::Request::SetBufferScale { scale } => {
                SurfaceData::<U, R>::with_data(&surface, |d| d.buffer_scale = scale);
            }
            wl_surface::Request::DamageBuffer { x, y, width, height } => {
                SurfaceData::<U, R>::with_data(&surface, |d| {
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

fn implement_surface<U, R, Impl>(
    surface: NewResource<wl_surface::WlSurface>,
    log: ::slog::Logger,
    implem: Rc<RefCell<Impl>>,
) -> wl_surface::WlSurface
where
    U: Default + 'static,
    R: Default + 'static,
    Impl: FnMut(SurfaceEvent, wl_surface::WlSurface, CompositorToken<U, R>) + 'static,
{
    let surface = surface.implement_closure(
        {
            let mut implem = SurfaceImplem::make(log, implem);
            move |req, surface| implem.receive_surface_request(req, surface)
        },
        Some(|surface| SurfaceData::<U, R>::cleanup(&surface)),
        SurfaceData::<U, R>::new(),
    );
    surface
}

/*
 * wl_region
 */

fn region_implem(request: wl_region::Request, region: wl_region::WlRegion) {
    let attributes_mutex = region.as_ref().user_data::<Mutex<RegionAttributes>>().unwrap();
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

fn implement_region(region: NewResource<wl_region::WlRegion>) -> wl_region::WlRegion {
    region.implement_closure(
        region_implem,
        None::<fn(_)>,
        Mutex::new(RegionAttributes::default()),
    )
}

/*
 * wl_subcompositor
 */

pub(crate) fn implement_subcompositor<U, R>(
    subcompositor: NewResource<wl_subcompositor::WlSubcompositor>,
) -> wl_subcompositor::WlSubcompositor
where
    R: RoleType + Role<SubsurfaceRole> + 'static,
    U: 'static,
{
    subcompositor.implement_closure(
        move |request, subcompositor| match request {
            wl_subcompositor::Request::GetSubsurface { id, surface, parent } => {
                if let Err(()) = SurfaceData::<U, R>::set_parent(&surface, &parent) {
                    subcompositor.as_ref().post_error(
                        wl_subcompositor::Error::BadSurface as u32,
                        "Surface already has a role.".into(),
                    );
                    return;
                }
                implement_subsurface::<U, R>(id, surface.clone());
            }
            wl_subcompositor::Request::Destroy => {}
            _ => unreachable!(),
        },
        None::<fn(_)>,
        (),
    )
}

/*
 * wl_subsurface
 */

fn with_subsurface_attributes<U, R, F>(subsurface: &wl_subsurface::WlSubsurface, f: F)
where
    F: FnOnce(&mut SubsurfaceRole),
    U: 'static,
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    let surface = subsurface.as_ref().user_data::<wl_surface::WlSurface>().unwrap();
    SurfaceData::<U, R>::with_role_data::<SubsurfaceRole, _, _>(surface, |d| f(d))
        .expect("The surface does not have a subsurface role while it has a wl_subsurface?!");
}

fn implement_subsurface<U, R>(
    subsurface: NewResource<wl_subsurface::WlSubsurface>,
    surface: wl_surface::WlSurface,
) -> wl_subsurface::WlSubsurface
where
    U: 'static,
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    subsurface.implement_closure(
        |request, subsurface| {
            match request {
                wl_subsurface::Request::SetPosition { x, y } => {
                    with_subsurface_attributes::<U, R, _>(&subsurface, |attrs| {
                        attrs.location = (x, y);
                    })
                }
                wl_subsurface::Request::PlaceAbove { sibling } => {
                    let surface = subsurface.as_ref().user_data::<wl_surface::WlSurface>().unwrap();
                    if let Err(()) = SurfaceData::<U, R>::reorder(surface, Location::After, &sibling) {
                        subsurface.as_ref().post_error(
                            wl_subsurface::Error::BadSurface as u32,
                            "Provided surface is not a sibling or parent.".into(),
                        )
                    }
                }
                wl_subsurface::Request::PlaceBelow { sibling } => {
                    let surface = subsurface.as_ref().user_data::<wl_surface::WlSurface>().unwrap();
                    if let Err(()) = SurfaceData::<U, R>::reorder(surface, Location::Before, &sibling) {
                        subsurface.as_ref().post_error(
                            wl_subsurface::Error::BadSurface as u32,
                            "Provided surface is not a sibling or parent.".into(),
                        )
                    }
                }
                wl_subsurface::Request::SetSync => {
                    with_subsurface_attributes::<U, R, _>(&subsurface, |attrs| {
                        attrs.sync = true;
                    })
                }
                wl_subsurface::Request::SetDesync => {
                    with_subsurface_attributes::<U, R, _>(&subsurface, |attrs| {
                        attrs.sync = false;
                    })
                }
                wl_subsurface::Request::Destroy => {
                    // Our destructor already handles it
                }
                _ => unreachable!(),
            }
        },
        Some(|subsurface| destroy_subsurface::<U, R>(&subsurface)),
        surface,
    )
}

fn destroy_subsurface<U, R>(subsurface: &wl_subsurface::WlSubsurface)
where
    U: 'static,
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    let surface = subsurface.as_ref().user_data::<wl_surface::WlSurface>().unwrap();
    if surface.as_ref().is_alive() {
        SurfaceData::<U, R>::unset_parent(&surface);
    }
}
