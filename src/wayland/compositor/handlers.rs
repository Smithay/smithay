use super::{CompositorToken, Damage, Rectangle, RectangleKind, Role, RoleType, SubsurfaceRole, SurfaceEvent};
use super::region::RegionData;
use super::tree::{Location, SurfaceData};
use std::cell::RefCell;
use std::rc::Rc;
use wayland_server::{LoopToken, NewResource, Resource};
use wayland_server::commons::Implementation;
use wayland_server::protocol::{wl_compositor, wl_region, wl_subcompositor, wl_subsurface, wl_surface};

/*
 * wl_compositor
 */

pub(crate) fn implement_compositor<U, R, Impl>(
    compositor: NewResource<wl_compositor::WlCompositor>,
    token: LoopToken,
    log: ::slog::Logger,
    implem: Rc<RefCell<Impl>>,
) -> Resource<wl_compositor::WlCompositor>
where
    U: Default + 'static,
    R: Default + 'static,
    Impl: Implementation<(Resource<wl_surface::WlSurface>, CompositorToken<U, R>), SurfaceEvent> + 'static,
{
    let my_token = token.clone();
    compositor.implement_nonsend(
        move |request, _compositor| match request {
            wl_compositor::Request::CreateSurface { id } => {
                trace!(log, "Creating a new wl_surface.");
                implement_surface(id, &token, log.clone(), implem.clone());
            }
            wl_compositor::Request::CreateRegion { id } => {
                trace!(log, "Creating a new wl_region.");
                implement_region(id, &token);
            }
        },
        None::<fn(_, _)>,
        &my_token,
    )
}

/*
 * wl_surface
 */

// Internal implementation data of surfaces
pub(crate) struct SurfaceImplem<U, R> {
    log: ::slog::Logger,
    implem:
        Rc<RefCell<Implementation<(Resource<wl_surface::WlSurface>, CompositorToken<U, R>), SurfaceEvent>>>,
}

impl<U, R> SurfaceImplem<U, R> {
    fn make<Impl>(log: ::slog::Logger, implem: Rc<RefCell<Impl>>) -> SurfaceImplem<U, R>
    where
        Impl: Implementation<(Resource<wl_surface::WlSurface>, CompositorToken<U, R>), SurfaceEvent>
            + 'static,
    {
        SurfaceImplem {
            log,
            implem,
        }
    }
}

impl<U, R> Implementation<Resource<wl_surface::WlSurface>, wl_surface::Request> for SurfaceImplem<U, R>
where
    U: 'static,
    R: 'static,
{
    fn receive(&mut self, req: wl_surface::Request, surface: Resource<wl_surface::WlSurface>) {
        match req {
            wl_surface::Request::Attach { buffer, x, y } => unsafe {
                SurfaceData::<U, R>::with_data(&surface, |d| {
                    d.buffer = Some(buffer.map(|b| (b.clone(), (x, y))))
                });
            },
            wl_surface::Request::Damage {
                x,
                y,
                width,
                height,
            } => unsafe {
                SurfaceData::<U, R>::with_data(&surface, |d| {
                    d.damage = Damage::Surface(Rectangle {
                        x,
                        y,
                        width,
                        height,
                    })
                });
            },
            wl_surface::Request::Frame { callback } => {
                let mut user_impl = self.implem.borrow_mut();
                trace!(self.log, "Calling user implementation for wl_surface.frame");
                user_impl.receive(
                    SurfaceEvent::Frame { callback },
                    (surface, CompositorToken::make()),
                );
            }
            wl_surface::Request::SetOpaqueRegion { region } => unsafe {
                let attributes = region.map(|r| RegionData::get_attributes(&r));
                SurfaceData::<U, R>::with_data(&surface, |d| d.opaque_region = attributes);
            },
            wl_surface::Request::SetInputRegion { region } => unsafe {
                let attributes = region.map(|r| RegionData::get_attributes(&r));
                SurfaceData::<U, R>::with_data(&surface, |d| d.input_region = attributes);
            },
            wl_surface::Request::Commit => {
                let mut user_impl = self.implem.borrow_mut();
                trace!(
                    self.log,
                    "Calling user implementation for wl_surface.commit"
                );
                user_impl.receive(SurfaceEvent::Commit, (surface, CompositorToken::make()));
            }
            wl_surface::Request::SetBufferTransform { transform } => unsafe {
                SurfaceData::<U, R>::with_data(&surface, |d| d.buffer_transform = transform);
            },
            wl_surface::Request::SetBufferScale { scale } => unsafe {
                SurfaceData::<U, R>::with_data(&surface, |d| d.buffer_scale = scale);
            },
            wl_surface::Request::DamageBuffer {
                x,
                y,
                width,
                height,
            } => unsafe {
                SurfaceData::<U, R>::with_data(&surface, |d| {
                    d.damage = Damage::Buffer(Rectangle {
                        x,
                        y,
                        width,
                        height,
                    })
                });
            },
            wl_surface::Request::Destroy => {
                // All is already handled by our destructor
            }
        }
    }
}

fn implement_surface<U, R, Impl>(
    surface: NewResource<wl_surface::WlSurface>,
    token: &LoopToken,
    log: ::slog::Logger,
    implem: Rc<RefCell<Impl>>,
) -> Resource<wl_surface::WlSurface>
where
    U: Default + 'static,
    R: Default + 'static,
    Impl: Implementation<(Resource<wl_surface::WlSurface>, CompositorToken<U, R>), SurfaceEvent> + 'static,
{
    let surface = surface.implement_nonsend(
        SurfaceImplem::make(log, implem),
        Some(|surface, _| unsafe {
            SurfaceData::<U, R>::cleanup(&surface);
        }),
        token,
    );
    unsafe {
        SurfaceData::<U, R>::init(&surface);
    }
    surface
}

/*
 * wl_region
 */

pub(crate) struct RegionImplem;

impl Implementation<Resource<wl_region::WlRegion>, wl_region::Request> for RegionImplem {
    fn receive(&mut self, request: wl_region::Request, region: Resource<wl_region::WlRegion>) {
        unsafe {
            match request {
                wl_region::Request::Add {
                    x,
                    y,
                    width,
                    height,
                } => RegionData::add_rectangle(
                    &region,
                    RectangleKind::Add,
                    Rectangle {
                        x,
                        y,
                        width,
                        height,
                    },
                ),
                wl_region::Request::Subtract {
                    x,
                    y,
                    width,
                    height,
                } => RegionData::add_rectangle(
                    &region,
                    RectangleKind::Subtract,
                    Rectangle {
                        x,
                        y,
                        width,
                        height,
                    },
                ),
                wl_region::Request::Destroy => {
                    // all is handled by our destructor
                }
            }
        }
    }
}

fn implement_region(
    region: NewResource<wl_region::WlRegion>,
    token: &LoopToken,
) -> Resource<wl_region::WlRegion> {
    let region = region.implement_nonsend(
        RegionImplem,
        Some(|region, _| unsafe { RegionData::cleanup(&region) }),
        token,
    );
    unsafe {
        RegionData::init(&region);
    }
    region
}

/*
 * wl_subcompositor
 */

pub(crate) fn implement_subcompositor<U, R>(
    subcompositor: NewResource<wl_subcompositor::WlSubcompositor>,
    token: LoopToken,
) -> Resource<wl_subcompositor::WlSubcompositor>
where
    R: RoleType + Role<SubsurfaceRole> + 'static,
    U: 'static,
{
    let my_token = token.clone();
    subcompositor.implement_nonsend(
        move |request, subcompositor: Resource<_>| match request {
            wl_subcompositor::Request::GetSubsurface {
                id,
                surface,
                parent,
            } => {
                if let Err(()) = unsafe { SurfaceData::<U, R>::set_parent(&surface, &parent) } {
                    subcompositor.post_error(
                        wl_subcompositor::Error::BadSurface as u32,
                        "Surface already has a role.".into(),
                    );
                    return;
                }
                let subsurface = implement_subsurface::<U, R>(id, &token);
                subsurface.set_user_data(Box::into_raw(Box::new(surface.clone())) as *mut ());
            }
            wl_subcompositor::Request::Destroy => {}
        },
        None::<fn(_, _)>,
        &my_token,
    )
}

/*
 * wl_subsurface
 */

unsafe fn with_subsurface_attributes<U, R, F>(subsurface: &Resource<wl_subsurface::WlSubsurface>, f: F)
where
    F: FnOnce(&mut SubsurfaceRole),
    U: 'static,
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    let ptr = subsurface.get_user_data();
    let surface = &*(ptr as *mut Resource<wl_surface::WlSurface>);
    SurfaceData::<U, R>::with_role_data::<SubsurfaceRole, _, _>(surface, |d| f(d))
        .expect("The surface does not have a subsurface role while it has a wl_subsurface?!");
}

fn implement_subsurface<U, R>(
    subsurface: NewResource<wl_subsurface::WlSubsurface>,
    token: &LoopToken,
) -> Resource<wl_subsurface::WlSubsurface>
where
    U: 'static,
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    subsurface.implement_nonsend(
        |request, subsurface| unsafe {
            match request {
                wl_subsurface::Request::SetPosition { x, y } => {
                    with_subsurface_attributes::<U, R, _>(&subsurface, |attrs| {
                        attrs.location = (x, y);
                    })
                }
                wl_subsurface::Request::PlaceAbove { sibling } => {
                    let surface = &*(subsurface.get_user_data() as *mut Resource<wl_surface::WlSurface>);
                    if let Err(()) = SurfaceData::<U, R>::reorder(surface, Location::After, &sibling) {
                        subsurface.post_error(
                            wl_subsurface::Error::BadSurface as u32,
                            "Provided surface is not a sibling or parent.".into(),
                        )
                    }
                }
                wl_subsurface::Request::PlaceBelow { sibling } => {
                    let surface = &*(subsurface.get_user_data() as *mut Resource<wl_surface::WlSurface>);
                    if let Err(()) = SurfaceData::<U, R>::reorder(surface, Location::Before, &sibling) {
                        subsurface.post_error(
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
            }
        },
        Some(|subsurface, _| unsafe {
            destroy_subsurface::<U, R>(&subsurface);
        }),
        token,
    )
}

unsafe fn destroy_subsurface<U, R>(subsurface: &Resource<wl_subsurface::WlSubsurface>)
where
    U: 'static,
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    let ptr = subsurface.get_user_data();
    subsurface.set_user_data(::std::ptr::null_mut());
    let surface = Box::from_raw(ptr as *mut Resource<wl_surface::WlSurface>);
    if surface.is_alive() {
        SurfaceData::<U, R>::unset_parent(&surface);
    }
}
