use super::{CompositorToken, Damage, Rectangle, RectangleKind, Role, RoleType, SubsurfaceRole,
            SurfaceUserImplementation};
use super::region::RegionData;
use super::tree::{Location, SurfaceData};
use std::cell::RefCell;
use std::rc::Rc;
use wayland_server::{Client, EventLoopHandle, Liveness, Resource};
use wayland_server::protocol::{wl_compositor, wl_region, wl_subcompositor, wl_subsurface, wl_surface};

/*
 * wl_compositor
 */

pub(crate) fn compositor_bind<U, R, ID>(evlh: &mut EventLoopHandle, idata: &mut SurfaceIData<U, R, ID>,
                                        _: &Client, compositor: wl_compositor::WlCompositor)
where
    U: Default + 'static,
    R: Default + 'static,
    ID: 'static,
{
    trace!(idata.log, "Binding a new wl_compositor.");
    evlh.register(
        &compositor,
        compositor_implementation::<U, R, ID>(),
        idata.clone(),
        None,
    );
}

fn compositor_implementation<U, R, ID>() -> wl_compositor::Implementation<SurfaceIData<U, R, ID>>
where
    U: Default + 'static,
    R: Default + 'static,
    ID: 'static,
{
    wl_compositor::Implementation {
        create_surface: |evlh, idata, _, _, surface| {
            unsafe { SurfaceData::<U, R>::init(&surface) };
            evlh.register(
                &surface,
                surface_implementation::<U, R, ID>(),
                idata.clone(),
                Some(destroy_surface::<U, R>),
            );
        },
        create_region: |evlh, _, _, _, region| {
            unsafe { RegionData::init(&region) };
            evlh.register(&region, region_implementation(), (), Some(destroy_region));
        },
    }
}

/*
 * wl_surface
 */

/// Internal implementation data of surfaces
///
/// This type is only visible as type parameter of
/// the `Global` handle you are provided.
pub struct SurfaceIData<U, R, ID> {
    log: ::slog::Logger,
    implem: SurfaceUserImplementation<U, R, ID>,
    idata: Rc<RefCell<ID>>,
}

impl<U, R, ID> SurfaceIData<U, R, ID> {
    pub(crate) fn make(log: ::slog::Logger, implem: SurfaceUserImplementation<U, R, ID>, idata: ID)
                       -> SurfaceIData<U, R, ID> {
        SurfaceIData {
            log: log,
            implem: implem,
            idata: Rc::new(RefCell::new(idata)),
        }
    }
}

impl<U, R, ID> Clone for SurfaceIData<U, R, ID> {
    fn clone(&self) -> SurfaceIData<U, R, ID> {
        SurfaceIData {
            log: self.log.clone(),
            implem: self.implem.clone(),
            idata: self.idata.clone(),
        }
    }
}

pub(crate) fn surface_implementation<U: 'static, R: 'static, ID: 'static>(
    )
    -> wl_surface::Implementation<SurfaceIData<U, R, ID>>
{
    wl_surface::Implementation {
        attach: |_, _, _, surface, buffer, x, y| unsafe {
            SurfaceData::<U, R>::with_data(surface, |d| {
                d.buffer = Some(buffer.map(|b| (b.clone_unchecked(), (x, y))))
            });
        },
        damage: |_, _, _, surface, x, y, width, height| unsafe {
            SurfaceData::<U, R>::with_data(surface, |d| {
                d.damage = Damage::Surface(Rectangle {
                    x,
                    y,
                    width,
                    height,
                })
            });
        },
        frame: |evlh, idata, _, surface, callback| {
            let mut user_idata = idata.idata.borrow_mut();
            trace!(idata.log, "Calling user callback for wl_surface.frame");
            (idata.implem.frame)(
                evlh,
                &mut *user_idata,
                surface,
                callback,
                CompositorToken::make(),
            )
        },
        set_opaque_region: |_, _, _, surface, region| unsafe {
            let attributes = region.map(|r| RegionData::get_attributes(r));
            SurfaceData::<U, R>::with_data(surface, |d| d.opaque_region = attributes);
        },
        set_input_region: |_, _, _, surface, region| unsafe {
            let attributes = region.map(|r| RegionData::get_attributes(r));
            SurfaceData::<U, R>::with_data(surface, |d| d.input_region = attributes);
        },
        commit: |evlh, idata, _, surface| {
            let mut user_idata = idata.idata.borrow_mut();
            trace!(idata.log, "Calling user callback for wl_surface.commit");
            (idata.implem.commit)(evlh, &mut *user_idata, surface, CompositorToken::make())
        },
        set_buffer_transform: |_, _, _, surface, transform| unsafe {
            SurfaceData::<U, R>::with_data(surface, |d| d.buffer_transform = transform);
        },
        set_buffer_scale: |_, _, _, surface, scale| unsafe {
            SurfaceData::<U, R>::with_data(surface, |d| d.buffer_scale = scale);
        },
        damage_buffer: |_, _, _, surface, x, y, width, height| unsafe {
            SurfaceData::<U, R>::with_data(surface, |d| {
                d.damage = Damage::Buffer(Rectangle {
                    x,
                    y,
                    width,
                    height,
                })
            });
        },
        destroy: |_, _, _, _| {},
    }
}

fn destroy_surface<U: 'static, R: 'static>(surface: &wl_surface::WlSurface) {
    unsafe { SurfaceData::<U, R>::cleanup(surface) }
}

/*
 * wl_region
 */

pub(crate) fn region_implementation() -> wl_region::Implementation<()> {
    wl_region::Implementation {
        add: |_, _, _, region, x, y, width, height| {
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
        },
        subtract: |_, _, _, region, x, y, width, height| {
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
        },
        destroy: |_, _, _, _| {},
    }
}

fn destroy_region(region: &wl_region::WlRegion) {
    unsafe { RegionData::cleanup(region) };
}

/*
 * wl_subcompositor
 */

pub(crate) fn subcompositor_bind<U, R>(evlh: &mut EventLoopHandle, _: &mut (), _: &Client,
                                       subcompositor: wl_subcompositor::WlSubcompositor)
where
    R: RoleType + Role<SubsurfaceRole> + 'static,
    U: 'static,
{
    evlh.register(
        &subcompositor,
        subcompositor_implementation::<U, R>(),
        (),
        None,
    );
}

fn subcompositor_implementation<U, R>() -> wl_subcompositor::Implementation<()>
where
    R: RoleType + Role<SubsurfaceRole> + 'static,
    U: 'static,
{
    wl_subcompositor::Implementation {
        get_subsurface: |evlh, _, _, subcompositor, subsurface, surface, parent| {
            if let Err(()) = unsafe { SurfaceData::<U, R>::set_parent(surface, parent) } {
                subcompositor.post_error(
                    wl_subcompositor::Error::BadSurface as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }
            subsurface.set_user_data(Box::into_raw(Box::new(unsafe { surface.clone_unchecked() })) as *mut _);
            evlh.register(
                &subsurface,
                subsurface_implementation::<U, R>(),
                (),
                Some(destroy_subsurface::<U, R>),
            );
        },
        destroy: |_, _, _, _| {},
    }
}

/*
 * wl_subsurface
 */

unsafe fn with_subsurface_attributes<U, R, F>(subsurface: &wl_subsurface::WlSubsurface, f: F)
where
    F: FnOnce(&mut SubsurfaceRole),
    U: 'static,
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    let ptr = subsurface.get_user_data();
    let surface = &*(ptr as *mut wl_surface::WlSurface);
    SurfaceData::<U, R>::with_role_data::<SubsurfaceRole, _, _>(surface, |d| f(d))
        .expect("The surface does not have a subsurface role while it has a wl_subsurface?!");
}

fn subsurface_implementation<U, R>() -> wl_subsurface::Implementation<()>
where
    R: RoleType + Role<SubsurfaceRole> + 'static,
    U: 'static,
{
    wl_subsurface::Implementation {
        set_position: |_, _, _, subsurface, x, y| unsafe {
            with_subsurface_attributes::<U, R, _>(subsurface, |attrs| {
                attrs.x = x;
                attrs.y = y;
            });
        },
        place_above: |_, _, _, subsurface, sibling| unsafe {
            let ptr = subsurface.get_user_data();
            let surface = &*(ptr as *mut wl_surface::WlSurface);
            if let Err(()) = SurfaceData::<U, R>::reorder(surface, Location::After, sibling) {
                subsurface.post_error(
                    wl_subsurface::Error::BadSurface as u32,
                    "Provided surface is not a sibling or parent.".into(),
                );
            }
        },
        place_below: |_, _, _, subsurface, sibling| unsafe {
            let ptr = subsurface.get_user_data();
            let surface = &*(ptr as *mut wl_surface::WlSurface);
            if let Err(()) = SurfaceData::<U, R>::reorder(surface, Location::Before, sibling) {
                subsurface.post_error(
                    wl_subsurface::Error::BadSurface as u32,
                    "Provided surface is not a sibling or parent.".into(),
                );
            }
        },
        set_sync: |_, _, _, subsurface| unsafe {
            with_subsurface_attributes::<U, R, _>(subsurface, |attrs| {
                attrs.sync = true;
            });
        },
        set_desync: |_, _, _, subsurface| unsafe {
            with_subsurface_attributes::<U, R, _>(subsurface, |attrs| {
                attrs.sync = false;
            });
        },
        destroy: |_, _, _, _| {},
    }
}

fn destroy_subsurface<U, R>(subsurface: &wl_subsurface::WlSubsurface)
where
    U: 'static,
    R: RoleType + Role<SubsurfaceRole> + 'static,
{
    let ptr = subsurface.get_user_data();
    subsurface.set_user_data(::std::ptr::null_mut());
    unsafe {
        let surface = Box::from_raw(ptr as *mut wl_surface::WlSurface);
        if surface.status() == Liveness::Alive {
            SurfaceData::<U, R>::unset_parent(&surface);
        }
    }
}
