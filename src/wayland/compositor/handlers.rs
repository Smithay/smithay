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
    DataInit, DelegateDispatch, DelegateDispatchBase, DelegateGlobalDispatch, DelegateGlobalDispatchBase,
    DestructionNotify, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

use crate::utils::{Logical, Point};

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

impl DelegateGlobalDispatchBase<WlCompositor> for CompositorState {
    type GlobalData = ();
}

impl<D> DelegateGlobalDispatch<WlCompositor, D> for CompositorState
where
    D: GlobalDispatch<WlCompositor, GlobalData = ()>,
    D: Dispatch<WlCompositor, UserData = ()>,
    D: Dispatch<WlSurface, UserData = SurfaceUserData>,
    D: Dispatch<WlRegion, UserData = RegionUserData>,
    D: CompositorHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &mut DisplayHandle<'_>,
        _client: &wayland_server::Client,
        resource: New<WlCompositor>,
        _global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl DelegateDispatchBase<WlCompositor> for CompositorState {
    type UserData = ();
}

impl<D> DelegateDispatch<WlCompositor, D> for CompositorState
where
    D: Dispatch<WlCompositor, UserData = ()>,
    D: Dispatch<WlSurface, UserData = SurfaceUserData>,
    D: Dispatch<WlRegion, UserData = RegionUserData>,
    D: CompositorHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlCompositor,
        request: wl_compositor::Request,
        _data: &Self::UserData,
        _dhandle: &mut DisplayHandle<'_>,
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
    fn commit(&mut self, _cx: &mut DisplayHandle<'_>) -> Self {
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
    fn merge_into(self, into: &mut Self, cx: &mut DisplayHandle<'_>) {
        if self.buffer.is_some() {
            if let Some(BufferAssignment::NewBuffer { buffer, .. }) =
                std::mem::replace(&mut into.buffer, self.buffer)
            {
                buffer.release(cx);
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

/// User data for WlSurface
#[derive(Debug)]
pub struct SurfaceUserData {
    pub(crate) inner: Mutex<PrivateSurfaceData>,
}

impl DestructionNotify for SurfaceUserData {
    fn object_destroyed(
        &self,
        _client_id: wayland_server::backend::ClientId,
        object_id: wayland_server::backend::ObjectId,
    ) {
        PrivateSurfaceData::cleanup(self, object_id);
    }
}

impl DelegateDispatchBase<WlSurface> for CompositorState {
    type UserData = SurfaceUserData;
}

impl<D> DelegateDispatch<WlSurface, D> for CompositorState
where
    D: Dispatch<WlSurface, UserData = SurfaceUserData>,
    D: Dispatch<WlCallback, UserData = ()>,
    D: CompositorHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        surface: &WlSurface,
        request: wl_surface::Request,
        _data: &Self::UserData,
        handle: &mut DisplayHandle<'_>,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wl_surface::Request::Attach { buffer, x, y } => {
                PrivateSurfaceData::with_states(surface, |states| {
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
                PrivateSurfaceData::invoke_commit_hooks(surface);

                // is_alive check
                if handle.object_info(surface.id()).is_err() {
                    // the client was killed by a hook, abort
                    return;
                }

                PrivateSurfaceData::commit(surface, handle);
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
            wl_surface::Request::Destroy => {
                // All is already handled by our destructor
            }
            _ => unreachable!(),
        }
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

impl DestructionNotify for RegionUserData {
    fn object_destroyed(
        &self,
        _client_id: wayland_server::backend::ClientId,
        _object_id: wayland_server::backend::ObjectId,
    ) {
    }
}

impl DelegateDispatchBase<WlRegion> for CompositorState {
    type UserData = RegionUserData;
}

impl<D> DelegateDispatch<WlRegion, D> for CompositorState
where
    D: Dispatch<WlRegion, UserData = RegionUserData>,
    D: CompositorHandler,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlRegion,
        request: wl_region::Request,
        data: &Self::UserData,
        _dhandle: &mut DisplayHandle<'_>,
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

impl DelegateGlobalDispatchBase<WlSubcompositor> for CompositorState {
    type GlobalData = ();
}

impl<D> DelegateGlobalDispatch<WlSubcompositor, D> for CompositorState
where
    D: GlobalDispatch<WlSubcompositor, GlobalData = ()>,
    D: Dispatch<WlSubcompositor, UserData = ()>,
    D: Dispatch<WlSubsurface, UserData = SubsurfaceUserData>,
    D: CompositorHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &mut DisplayHandle<'_>,
        _client: &wayland_server::Client,
        resource: New<WlSubcompositor>,
        _global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl DelegateDispatchBase<WlSubcompositor> for CompositorState {
    type UserData = ();
}

impl<D> DelegateDispatch<WlSubcompositor, D> for CompositorState
where
    D: Dispatch<WlSubcompositor, UserData = ()>,
    D: Dispatch<WlSubsurface, UserData = SubsurfaceUserData>,
    D: CompositorHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        subcompositor: &WlSubcompositor,
        request: wl_subcompositor::Request,
        _data: &Self::UserData,
        handle: &mut DisplayHandle<'_>,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wl_subcompositor::Request::GetSubsurface { id, surface, parent } => {
                if let Err(AlreadyHasRole) = PrivateSurfaceData::set_parent(&surface, &parent) {
                    subcompositor.post_error(
                        handle,
                        wl_subcompositor::Error::BadSurface,
                        "Surface already has a role.",
                    );
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
                })
                .unwrap();
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

impl DestructionNotify for SubsurfaceUserData {
    fn object_destroyed(
        &self,
        _client_id: wayland_server::backend::ClientId,
        _object_id: wayland_server::backend::ObjectId,
    ) {
        // TODO
        // if surface.as_ref().is_alive() {
        PrivateSurfaceData::unset_parent(&self.surface);
        // }
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
    fn commit(&mut self, _cx: &mut DisplayHandle<'_>) -> Self {
        Self {
            location: self.location,
        }
    }

    fn merge_into(self, into: &mut Self, _cx: &mut DisplayHandle<'_>) {
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

impl DelegateDispatchBase<WlSubsurface> for CompositorState {
    type UserData = SubsurfaceUserData;
}

impl<D> DelegateDispatch<WlSubsurface, D> for CompositorState
where
    D: Dispatch<WlSubsurface, UserData = SubsurfaceUserData>,
    D: CompositorHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        subsurface: &WlSubsurface,
        request: wl_subsurface::Request,
        data: &Self::UserData,
        handle: &mut DisplayHandle<'_>,
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
                        handle,
                        wl_subsurface::Error::BadSurface,
                        "Provided surface is not a sibling or parent.",
                    )
                }
            }
            wl_subsurface::Request::PlaceBelow { sibling } => {
                if let Err(()) = PrivateSurfaceData::reorder(&data.surface, Location::Before, &sibling) {
                    subsurface.post_error(
                        handle,
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
}

impl DelegateDispatchBase<WlCallback> for CompositorState {
    type UserData = ();
}

impl<D> DelegateDispatch<WlCallback, D> for CompositorState
where
    D: Dispatch<WlCallback, UserData = ()>,
    D: CompositorHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _subsurface: &WlCallback,
        _request: wl_callback::Request,
        _data: &Self::UserData,
        _handle: &mut DisplayHandle<'_>,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }
}
