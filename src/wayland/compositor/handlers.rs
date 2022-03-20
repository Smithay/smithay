use std::{
    cell::RefCell,
    ops::Deref as _,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

use wayland_server::{
    protocol::{wl_compositor, wl_region, wl_subcompositor, wl_subsurface, wl_surface},
    DispatchData, Filter, Main,
};

use crate::utils::{Logical, Point};

use super::{
    cache::Cacheable,
    tree::{Location, PrivateSurfaceData},
    AlreadyHasRole, BufferAssignment, Damage, Rectangle, RectangleKind, RegionAttributes, SurfaceAttributes,
};

use slog::trace;

/*
 * wl_compositor
 */

pub(crate) fn implement_compositor<Impl>(
    compositor: Main<wl_compositor::WlCompositor>,
    log: ::slog::Logger,
    implem: Rc<RefCell<Impl>>,
) -> wl_compositor::WlCompositor
where
    Impl: for<'a> FnMut(wl_surface::WlSurface, DispatchData<'a>) + 'static,
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

type SurfaceImplemFn = dyn for<'a> FnMut(wl_surface::WlSurface, DispatchData<'a>);

// Internal implementation data of surfaces
pub(crate) struct SurfaceImplem {
    log: ::slog::Logger,
    implem: Rc<RefCell<SurfaceImplemFn>>,
}

impl SurfaceImplem {
    fn make<Impl>(log: ::slog::Logger, implem: Rc<RefCell<Impl>>) -> SurfaceImplem
    where
        Impl: for<'a> FnMut(wl_surface::WlSurface, DispatchData<'a>) + 'static,
    {
        SurfaceImplem { log, implem }
    }
}

impl SurfaceImplem {
    fn receive_surface_request(
        &mut self,
        req: wl_surface::Request,
        surface: wl_surface::WlSurface,
        ddata: DispatchData<'_>,
    ) {
        match req {
            wl_surface::Request::Attach { buffer, x, y } => {
                PrivateSurfaceData::with_states(&surface, |states| {
                    states.cached_state.pending::<SurfaceAttributes>().buffer = Some(match buffer {
                        Some(buffer) => BufferAssignment::NewBuffer {
                            buffer,
                            delta: (x, y).into(),
                        },
                        None => BufferAssignment::Removed,
                    })
                });
            }
            wl_surface::Request::Damage { x, y, width, height } => {
                PrivateSurfaceData::with_states(&surface, |states| {
                    states
                        .cached_state
                        .pending::<SurfaceAttributes>()
                        .damage
                        .push(Damage::Surface(Rectangle::from_loc_and_size(
                            (x, y),
                            (width, height),
                        )));
                });
            }
            wl_surface::Request::Frame { callback } => {
                PrivateSurfaceData::with_states(&surface, |states| {
                    states
                        .cached_state
                        .pending::<SurfaceAttributes>()
                        .frame_callbacks
                        .push((*callback).clone());
                });
            }
            wl_surface::Request::SetOpaqueRegion { region } => {
                let attributes = region.map(|r| {
                    let attributes_mutex = r.as_ref().user_data().get::<Mutex<RegionAttributes>>().unwrap();
                    attributes_mutex.lock().unwrap().clone()
                });
                PrivateSurfaceData::with_states(&surface, |states| {
                    states.cached_state.pending::<SurfaceAttributes>().opaque_region = attributes;
                });
            }
            wl_surface::Request::SetInputRegion { region } => {
                let attributes = region.map(|r| {
                    let attributes_mutex = r.as_ref().user_data().get::<Mutex<RegionAttributes>>().unwrap();
                    attributes_mutex.lock().unwrap().clone()
                });
                PrivateSurfaceData::with_states(&surface, |states| {
                    states.cached_state.pending::<SurfaceAttributes>().input_region = attributes;
                });
            }
            wl_surface::Request::Commit => {
                let mut user_impl = self.implem.borrow_mut();
                PrivateSurfaceData::invoke_commit_hooks(&surface);
                if !surface.as_ref().is_alive() {
                    // the client was killed by a hook, abort
                    return;
                }
                PrivateSurfaceData::commit(&surface);
                trace!(self.log, "Calling user implementation for wl_surface.commit");
                (&mut *user_impl)(surface, ddata);
            }
            wl_surface::Request::SetBufferTransform { transform } => {
                PrivateSurfaceData::with_states(&surface, |states| {
                    states
                        .cached_state
                        .pending::<SurfaceAttributes>()
                        .buffer_transform = transform;
                });
            }
            wl_surface::Request::SetBufferScale { scale } => {
                PrivateSurfaceData::with_states(&surface, |states| {
                    states.cached_state.pending::<SurfaceAttributes>().buffer_scale = scale;
                });
            }
            wl_surface::Request::DamageBuffer { x, y, width, height } => {
                PrivateSurfaceData::with_states(&surface, |states| {
                    states
                        .cached_state
                        .pending::<SurfaceAttributes>()
                        .damage
                        .push(Damage::Buffer(Rectangle::from_loc_and_size(
                            (x, y),
                            (width, height),
                        )))
                });
            }
            wl_surface::Request::Destroy => {
                // All is already handled by our destructor
            }
            _ => unreachable!(),
        }
    }
}

impl Cacheable for SurfaceAttributes {
    fn commit(&mut self) -> Self {
        SurfaceAttributes {
            buffer: self.buffer.take(),
            buffer_scale: self.buffer_scale,
            buffer_transform: self.buffer_transform,
            damage: std::mem::take(&mut self.damage),
            opaque_region: self.opaque_region.clone(),
            input_region: self.input_region.clone(),
            frame_callbacks: std::mem::take(&mut self.frame_callbacks),
        }
    }
    fn merge_into(self, into: &mut Self) {
        if self.buffer.is_some() {
            if let Some(BufferAssignment::Removed) = &self.buffer {
                into.damage.clear();
            }
            if let Some(BufferAssignment::NewBuffer { buffer, .. }) =
                std::mem::replace(&mut into.buffer, self.buffer)
            {
                let new_buffer = into.buffer.as_ref().and_then(|b| match b {
                    BufferAssignment::Removed => None,
                    BufferAssignment::NewBuffer { buffer, .. } => Some(buffer),
                });

                if Some(&buffer) != new_buffer {
                    buffer.release();
                }
            }
        }
        into.buffer_scale = self.buffer_scale;
        into.buffer_transform = self.buffer_transform;
        into.damage.extend(self.damage);
        into.opaque_region = self.opaque_region;
        into.input_region = self.input_region;
        into.frame_callbacks.extend(self.frame_callbacks);
    }
}

fn implement_surface<Impl>(
    surface: Main<wl_surface::WlSurface>,
    log: ::slog::Logger,
    implem: Rc<RefCell<Impl>>,
) -> wl_surface::WlSurface
where
    Impl: for<'a> FnMut(wl_surface::WlSurface, DispatchData<'a>) + 'static,
{
    surface.quick_assign({
        let mut implem = SurfaceImplem::make(log, implem);
        move |surface, req, ddata| implem.receive_surface_request(req, surface.deref().clone(), ddata)
    });
    surface.assign_destructor(Filter::new(|surface, _, _| PrivateSurfaceData::cleanup(&surface)));
    surface
        .as_ref()
        .user_data()
        .set_threadsafe(PrivateSurfaceData::new);
    PrivateSurfaceData::init(&surface);
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
        wl_region::Request::Add { x, y, width, height } => guard.rects.push((
            RectangleKind::Add,
            Rectangle::from_loc_and_size((x, y), (width, height)),
        )),
        wl_region::Request::Subtract { x, y, width, height } => guard.rects.push((
            RectangleKind::Subtract,
            Rectangle::from_loc_and_size((x, y), (width, height)),
        )),
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

pub(crate) fn implement_subcompositor(
    subcompositor: Main<wl_subcompositor::WlSubcompositor>,
) -> wl_subcompositor::WlSubcompositor {
    subcompositor.quick_assign(move |subcompositor, request, _| match request {
        wl_subcompositor::Request::GetSubsurface { id, surface, parent } => {
            if let Err(AlreadyHasRole) = PrivateSurfaceData::set_parent(&surface, &parent) {
                subcompositor.as_ref().post_error(
                    wl_subcompositor::Error::BadSurface as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }
            implement_subsurface(id, surface);
        }
        wl_subcompositor::Request::Destroy => {}
        _ => unreachable!(),
    });
    subcompositor.deref().clone()
}

/*
 * wl_subsurface
 */

/// The cached state associated with a subsurface
#[derive(Debug)]
pub struct SubsurfaceCachedState {
    /// Location of the top-left corner of this subsurface
    /// relative to its parent coordinate space
    pub location: Point<i32, Logical>,
}

impl Default for SubsurfaceCachedState {
    fn default() -> Self {
        SubsurfaceCachedState {
            location: (0, 0).into(),
        }
    }
}

impl Cacheable for SubsurfaceCachedState {
    fn commit(&mut self) -> Self {
        SubsurfaceCachedState {
            location: self.location,
        }
    }

    fn merge_into(self, into: &mut Self) {
        into.location = self.location;
    }
}

pub(crate) struct SubsurfaceState {
    pub(crate) sync: AtomicBool,
}

impl SubsurfaceState {
    fn new() -> SubsurfaceState {
        SubsurfaceState {
            sync: AtomicBool::new(true),
        }
    }
}

/// Check if a (sub)surface is effectively sync
pub fn is_effectively_sync(surface: &wl_surface::WlSurface) -> bool {
    let is_direct_sync = PrivateSurfaceData::with_states(surface, |state| {
        state
            .data_map
            .get::<SubsurfaceState>()
            .map(|s| s.sync.load(Ordering::Acquire))
            .unwrap_or(false)
    });
    if is_direct_sync {
        return true;
    }
    if let Some(parent) = PrivateSurfaceData::get_parent(surface) {
        is_effectively_sync(&parent)
    } else {
        false
    }
}

fn implement_subsurface(
    subsurface: Main<wl_subsurface::WlSubsurface>,
    surface: wl_surface::WlSurface,
) -> wl_subsurface::WlSubsurface {
    let data_surface = surface.clone();
    subsurface.quick_assign(move |subsurface, request, _| {
        match request {
            wl_subsurface::Request::SetPosition { x, y } => {
                PrivateSurfaceData::with_states(&surface, |state| {
                    state.cached_state.pending::<SubsurfaceCachedState>().location = (x, y).into();
                })
            }
            wl_subsurface::Request::PlaceAbove { sibling } => {
                let surface = subsurface
                    .as_ref()
                    .user_data()
                    .get::<wl_surface::WlSurface>()
                    .unwrap();
                if let Err(()) = PrivateSurfaceData::reorder(surface, Location::After, &sibling) {
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
                if let Err(()) = PrivateSurfaceData::reorder(surface, Location::Before, &sibling) {
                    subsurface.as_ref().post_error(
                        wl_subsurface::Error::BadSurface as u32,
                        "Provided surface is not a sibling or parent.".into(),
                    )
                }
            }
            wl_subsurface::Request::SetSync => PrivateSurfaceData::with_states(&surface, |state| {
                state
                    .data_map
                    .get::<SubsurfaceState>()
                    .unwrap()
                    .sync
                    .store(true, Ordering::Release);
            }),
            wl_subsurface::Request::SetDesync => PrivateSurfaceData::with_states(&surface, |state| {
                state
                    .data_map
                    .get::<SubsurfaceState>()
                    .unwrap()
                    .sync
                    .store(false, Ordering::Release);
            }),
            wl_subsurface::Request::Destroy => {
                // Our destructor already handles it
            }
            _ => unreachable!(),
        }
    });
    super::with_states(&data_surface, |states| {
        states.data_map.insert_if_missing_threadsafe(SubsurfaceState::new)
    })
    .unwrap();
    subsurface.assign_destructor(Filter::new(|subsurface, _, _| destroy_subsurface(&subsurface)));
    subsurface
        .as_ref()
        .user_data()
        .set_threadsafe(move || data_surface);
    subsurface.deref().clone()
}

fn destroy_subsurface(subsurface: &wl_subsurface::WlSubsurface) {
    let surface = subsurface
        .as_ref()
        .user_data()
        .get::<wl_surface::WlSurface>()
        .unwrap();
    if surface.as_ref().is_alive() {
        PrivateSurfaceData::unset_parent(surface);
    }
}
