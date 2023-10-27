use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use wayland_server::{
    protocol::{
        wl_callback::{self, WlCallback},
        wl_compositor::{self, WlCompositor},
        wl_region::{self, WlRegion},
        wl_subcompositor::{self, WlSubcompositor},
        wl_subsurface::{self, WlSubsurface},
        wl_surface::{self, WlSurface},
    },
    DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

use crate::utils::{
    alive_tracker::{AliveTracker, IsAlive},
    Logical, Point,
};

use super::{
    cache::Cacheable,
    tree::{Location, PrivateSurfaceData},
    AlreadyHasRole, BufferAssignment, CompositorHandler, CompositorState, Damage, Rectangle, RectangleKind,
    RegionAttributes, SurfaceAttributes,
};

use tracing::trace;

/*
 * wl_compositor
 */

impl<D> GlobalDispatch<WlCompositor, (), D> for CompositorState
where
    D: GlobalDispatch<WlCompositor, ()>,
    D: Dispatch<WlCompositor, ()>,
    D: Dispatch<WlSurface, SurfaceUserData>,
    D: Dispatch<WlRegion, RegionUserData>,
    D: CompositorHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<WlCompositor>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<WlCompositor, (), D> for CompositorState
where
    D: Dispatch<WlCompositor, ()>,
    D: Dispatch<WlSurface, SurfaceUserData>,
    D: Dispatch<WlRegion, RegionUserData>,
    D: CompositorHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlCompositor,
        request: wl_compositor::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                trace!(id = ?id, "Creating a new wl_surface");

                let surface = data_init.init(
                    id,
                    SurfaceUserData {
                        inner: PrivateSurfaceData::new(),
                        alive_tracker: Default::default(),
                        user_state_type: (std::any::TypeId::of::<D>(), std::any::type_name::<D>()),
                    },
                );

                state.compositor_state().surfaces.push(surface.clone());

                PrivateSurfaceData::init(&surface);
                state.new_surface(&surface);
            }
            wl_compositor::Request::CreateRegion { id } => {
                trace!(id = ?id, "Creating a new wl_region");

                data_init.init(
                    id,
                    RegionUserData {
                        inner: Default::default(),
                    },
                );
            }
            _ => unreachable!(),
        }
    }
}

/*
 * wl_surface
 */

impl Cacheable for SurfaceAttributes {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        SurfaceAttributes {
            buffer: self.buffer.take(),
            buffer_delta: self.buffer_delta.take(),
            buffer_scale: self.buffer_scale,
            buffer_transform: self.buffer_transform,
            damage: std::mem::take(&mut self.damage),
            opaque_region: self.opaque_region.clone(),
            input_region: self.input_region.clone(),
            frame_callbacks: std::mem::take(&mut self.frame_callbacks),
        }
    }
    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        if self.buffer.is_some() {
            if let Some(BufferAssignment::NewBuffer(buffer)) =
                std::mem::replace(&mut into.buffer, self.buffer)
            {
                let new_buffer = into.buffer.as_ref().and_then(|b| match b {
                    BufferAssignment::Removed => None,
                    BufferAssignment::NewBuffer(buffer) => Some(buffer),
                });

                if Some(&buffer) != new_buffer {
                    buffer.release();
                }
            }
        }
        into.buffer_delta = self.buffer_delta;
        into.buffer_scale = self.buffer_scale;
        into.buffer_transform = self.buffer_transform;
        into.damage.extend(self.damage);
        into.opaque_region = self.opaque_region;
        into.input_region = self.input_region;
        into.frame_callbacks.extend(self.frame_callbacks);
    }
}

/// User data for WlSurface
#[derive(Debug)]
pub struct SurfaceUserData {
    pub(crate) inner: Mutex<PrivateSurfaceData>,
    alive_tracker: AliveTracker,
    pub(super) user_state_type: (std::any::TypeId, &'static str),
}

impl<D> Dispatch<WlSurface, SurfaceUserData, D> for CompositorState
where
    D: Dispatch<WlSurface, SurfaceUserData>,
    D: Dispatch<WlCallback, ()>,
    D: CompositorHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        surface: &WlSurface,
        request: wl_surface::Request,
        _data: &SurfaceUserData,
        handle: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wl_surface::Request::Attach { buffer, x, y } => {
                let offset: Point<i32, Logical> = (x, y).into();
                let offset = (x != 0 || y != 0).then_some(offset);

                // If version predates 5 just use the offset
                // Otherwise error out and use None
                let offset = if surface.version() < 5 {
                    offset
                } else {
                    if offset.is_some() {
                        surface.post_error(
                            wl_surface::Error::InvalidOffset,
                            "Passing non-zero x,y is protocol violation since versions 5",
                        );
                    }

                    None
                };

                PrivateSurfaceData::with_states(surface, |states| {
                    let mut pending = states.cached_state.pending::<SurfaceAttributes>();

                    // Let's set the offset here in case it is supported and non-zero
                    if offset.is_some() {
                        pending.buffer_delta = offset;
                    }

                    pending.buffer = Some(match buffer {
                        Some(buffer) => BufferAssignment::NewBuffer(buffer),
                        None => BufferAssignment::Removed,
                    })
                });
            }
            wl_surface::Request::Damage { x, y, width, height } => {
                PrivateSurfaceData::with_states(surface, |states| {
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
                let callback = data_init.init(callback, ());

                PrivateSurfaceData::with_states(surface, |states| {
                    states
                        .cached_state
                        .pending::<SurfaceAttributes>()
                        .frame_callbacks
                        .push(callback.clone());
                });
            }
            wl_surface::Request::SetOpaqueRegion { region } => {
                let attributes = region.map(|r| {
                    let attributes_mutex = &r.data::<RegionUserData>().unwrap().inner;
                    attributes_mutex.lock().unwrap().clone()
                });
                PrivateSurfaceData::with_states(surface, |states| {
                    states.cached_state.pending::<SurfaceAttributes>().opaque_region = attributes;
                });
            }
            wl_surface::Request::SetInputRegion { region } => {
                let attributes = region.map(|r| {
                    let attributes_mutex = &r.data::<RegionUserData>().unwrap().inner;
                    attributes_mutex.lock().unwrap().clone()
                });
                PrivateSurfaceData::with_states(surface, |states| {
                    states.cached_state.pending::<SurfaceAttributes>().input_region = attributes;
                });
            }
            wl_surface::Request::Commit => {
                PrivateSurfaceData::invoke_pre_commit_hooks(state, handle, surface);

                PrivateSurfaceData::commit(surface, handle, state);
            }
            wl_surface::Request::SetBufferTransform { transform } => {
                if let WEnum::Value(transform) = transform {
                    PrivateSurfaceData::with_states(surface, |states| {
                        states
                            .cached_state
                            .pending::<SurfaceAttributes>()
                            .buffer_transform = transform;
                    });
                }
            }
            wl_surface::Request::SetBufferScale { scale } => {
                PrivateSurfaceData::with_states(surface, |states| {
                    states.cached_state.pending::<SurfaceAttributes>().buffer_scale = scale;
                });
            }
            wl_surface::Request::DamageBuffer { x, y, width, height } => {
                PrivateSurfaceData::with_states(surface, |states| {
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
            wl_surface::Request::Offset { x, y } => {
                PrivateSurfaceData::with_states(surface, |states| {
                    states.cached_state.pending::<SurfaceAttributes>().buffer_delta = Some((x, y).into());
                });
            }
            wl_surface::Request::Destroy => {
                // All is already handled by our destructor
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut D,
        _client_id: wayland_server::backend::ClientId,
        surface: &WlSurface,
        data: &SurfaceUserData,
    ) {
        // We let the destruction hooks run first and then tell the compositor handler the surface was
        // destroyed.
        data.alive_tracker.destroy_notify();
        state.destroyed(surface);

        // Remove the surface after the callback is invoked.
        state
            .compositor_state()
            .surfaces
            .retain(|s| s.id() != surface.id());
        PrivateSurfaceData::cleanup(state, data, surface.id());
    }
}

impl IsAlive for WlSurface {
    fn alive(&self) -> bool {
        let data: &SurfaceUserData = self.data().unwrap();
        data.alive_tracker.alive()
    }
}

/*
 * wl_region
 */

/// User data of WlRegion
#[derive(Debug)]
pub struct RegionUserData {
    pub(crate) inner: Mutex<RegionAttributes>,
}

impl<D> Dispatch<WlRegion, RegionUserData, D> for CompositorState
where
    D: Dispatch<WlRegion, RegionUserData>,
    D: CompositorHandler,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlRegion,
        request: wl_region::Request,
        data: &RegionUserData,
        _dhandle: &DisplayHandle,
        _init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let mut guard = data.inner.lock().unwrap();
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
}

/*
 * wl_subcompositor
 */

impl<D> GlobalDispatch<WlSubcompositor, (), D> for CompositorState
where
    D: GlobalDispatch<WlSubcompositor, ()>,
    D: Dispatch<WlSubcompositor, ()>,
    D: Dispatch<WlSubsurface, SubsurfaceUserData>,
    D: CompositorHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<WlSubcompositor>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<WlSubcompositor, (), D> for CompositorState
where
    D: Dispatch<WlSubcompositor, ()>,
    D: Dispatch<WlSubsurface, SubsurfaceUserData>,
    D: CompositorHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        subcompositor: &WlSubcompositor,
        request: wl_subcompositor::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wl_subcompositor::Request::GetSubsurface { id, surface, parent } => {
                if let Err(AlreadyHasRole) = PrivateSurfaceData::set_parent(&surface, &parent) {
                    subcompositor
                        .post_error(wl_subcompositor::Error::BadSurface, "Surface already has a role.");
                    return;
                }

                data_init.init(
                    id,
                    SubsurfaceUserData {
                        surface: surface.clone(),
                    },
                );

                super::with_states(&surface, |states| {
                    states.data_map.insert_if_missing_threadsafe(SubsurfaceState::new)
                });

                state.new_subsurface(&surface, &parent);
            }
            wl_subcompositor::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

/*
 * wl_subsurface
 */

/// User data of WlSubsurface
#[derive(Debug)]
pub struct SubsurfaceUserData {
    surface: WlSurface,
}

impl SubsurfaceUserData {
    /// Returns the surface for this subsurface (not to be confused with the parent surface).
    pub fn surface(&self) -> &WlSurface {
        &self.surface
    }
}
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
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        Self {
            location: self.location,
        }
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
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

impl<D> Dispatch<WlSubsurface, SubsurfaceUserData, D> for CompositorState
where
    D: Dispatch<WlSubsurface, SubsurfaceUserData>,
    D: CompositorHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        subsurface: &WlSubsurface,
        request: wl_subsurface::Request,
        data: &SubsurfaceUserData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wl_subsurface::Request::SetPosition { x, y } => {
                PrivateSurfaceData::with_states(&data.surface, |state| {
                    state.cached_state.pending::<SubsurfaceCachedState>().location = (x, y).into();
                })
            }
            wl_subsurface::Request::PlaceAbove { sibling } => {
                if let Err(()) = PrivateSurfaceData::reorder(&data.surface, Location::After, &sibling) {
                    subsurface.post_error(
                        wl_subsurface::Error::BadSurface,
                        "Provided surface is not a sibling or parent.",
                    )
                }
            }
            wl_subsurface::Request::PlaceBelow { sibling } => {
                if let Err(()) = PrivateSurfaceData::reorder(&data.surface, Location::Before, &sibling) {
                    subsurface.post_error(
                        wl_subsurface::Error::BadSurface,
                        "Provided surface is not a sibling or parent.",
                    )
                }
            }
            wl_subsurface::Request::SetSync => PrivateSurfaceData::with_states(&data.surface, |state| {
                state
                    .data_map
                    .get::<SubsurfaceState>()
                    .unwrap()
                    .sync
                    .store(true, Ordering::Release);
            }),
            wl_subsurface::Request::SetDesync => PrivateSurfaceData::with_states(&data.surface, |state| {
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
    }

    fn destroyed(
        _state: &mut D,
        _client_id: wayland_server::backend::ClientId,
        _object: &WlSubsurface,
        data: &SubsurfaceUserData,
    ) {
        PrivateSurfaceData::unset_parent(&data.surface);
        PrivateSurfaceData::with_states(&data.surface, |state| {
            state
                .data_map
                .get::<SubsurfaceState>()
                .unwrap()
                .sync
                .store(true, Ordering::Release);
            *state.cached_state.pending::<SubsurfaceCachedState>() = Default::default();
            *state.cached_state.current::<SubsurfaceCachedState>() = Default::default();
        });
    }
}

impl<D> Dispatch<WlCallback, (), D> for CompositorState
where
    D: Dispatch<WlCallback, ()>,
    D: CompositorHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _subsurface: &WlCallback,
        _request: wl_callback::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }
}
