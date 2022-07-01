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
    DataInit, DelegateDispatch, DelegateGlobalDispatch, Dispatch, DisplayHandle, GlobalDispatch, New,
    Resource, WEnum,
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

use slog::trace;

/*
 * wl_compositor
 */

impl<D> DelegateGlobalDispatch<WlCompositor, (), D> for CompositorState
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

impl<D> DelegateDispatch<WlCompositor, (), D> for CompositorState
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
        let log = &state.compositor_state().log;
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                trace!(log, "Creating a new wl_surface.");

                let surface = data_init.init(
                    id,
                    SurfaceUserData {
                        inner: PrivateSurfaceData::new(),
                        alive_tracker: Default::default(),
                    },
                );
                PrivateSurfaceData::init(&surface);
            }
            wl_compositor::Request::CreateRegion { id } => {
                trace!(log, "Creating a new wl_region.");

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
        self.has_commited_buffer = match self.buffer {
            Some(BufferAssignment::NewBuffer(_)) => true,
            Some(BufferAssignment::Removed) => false,
            None => self.has_commited_buffer,
        };
        SurfaceAttributes {
            buffer: self.buffer.take(),
            buffer_delta: self.buffer_delta.take(),
            buffer_scale: self.buffer_scale,
            buffer_transform: self.buffer_transform,
            damage: std::mem::take(&mut self.damage),
            opaque_region: self.opaque_region.clone(),
            input_region: self.input_region.clone(),
            frame_callbacks: std::mem::take(&mut self.frame_callbacks),
            has_commited_buffer: self.has_commited_buffer,
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
}

impl<D> DelegateDispatch<WlSurface, SurfaceUserData, D> for CompositorState
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
                if !crate::wayland::shell::xdg::ensure_configured_if_xdg(surface) {
                    return;
                }

                let offset: Point<i32, Logical> = (x, y).into();
                let offset = (x != 0 || y != 0).then(|| offset);

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
                PrivateSurfaceData::invoke_pre_commit_hooks(handle, surface);

                PrivateSurfaceData::commit(surface, handle);

                PrivateSurfaceData::invoke_post_commit_hooks(handle, surface);

                trace!(
                    state.compositor_state().log,
                    "Calling user implementation for wl_surface.commit"
                );

                state.commit(handle, surface);
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
        _state: &mut D,
        _client_id: wayland_server::backend::ClientId,
        object_id: wayland_server::backend::ObjectId,
        data: &SurfaceUserData,
    ) {
        data.alive_tracker.destroy_notify();
        PrivateSurfaceData::cleanup(data, object_id);
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

impl<D> DelegateDispatch<WlRegion, RegionUserData, D> for CompositorState
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

impl<D> DelegateGlobalDispatch<WlSubcompositor, (), D> for CompositorState
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

impl<D> DelegateDispatch<WlSubcompositor, (), D> for CompositorState
where
    D: Dispatch<WlSubcompositor, ()>,
    D: Dispatch<WlSubsurface, SubsurfaceUserData>,
    D: CompositorHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
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

impl<D> DelegateDispatch<WlSubsurface, SubsurfaceUserData, D> for CompositorState
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
        _object_id: wayland_server::backend::ObjectId,
        data: &SubsurfaceUserData,
    ) {
        // TODO
        // if surface.as_ref().is_alive() {
        PrivateSurfaceData::unset_parent(&data.surface);
        // }
    }
}

impl<D> DelegateDispatch<WlCallback, (), D> for CompositorState
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
