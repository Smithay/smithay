use crate::wayland::background_effect::{BackgroundEffectState, BackgroundEffectSurfaceData};
use crate::wayland::{
    background_effect::{BackgroundEffectSurfaceCachedState, BackgroundEffectSurfaceUserData},
    compositor::with_states,
};
use wayland_protocols::ext::background_effect::v1::server::{
    ext_background_effect_manager_v1::{
        Capability, Error as ManagerError, ExtBackgroundEffectManagerV1, Request as ManagerRequest,
    },
    ext_background_effect_surface_v1::{
        Error as SurfaceError, ExtBackgroundEffectSurfaceV1, Request as SurfaceRequest,
    },
};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

// GlobalDispatch for ext_background_effect_manager_v1
impl<D> GlobalDispatch<ExtBackgroundEffectManagerV1, (), D> for BackgroundEffectState
where
    D: GlobalDispatch<ExtBackgroundEffectManagerV1, ()>,
    D: Dispatch<ExtBackgroundEffectManagerV1, ()>,
    D: Dispatch<ExtBackgroundEffectSurfaceV1, BackgroundEffectSurfaceUserData>,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtBackgroundEffectManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        let manager = data_init.init(resource, ());
        // For now, always advertise blur capability
        manager.capabilities(Capability::Blur);
    }
}

// Dispatch for ext_background_effect_manager_v1
impl<D> Dispatch<ExtBackgroundEffectManagerV1, (), D> for BackgroundEffectState
where
    D: Dispatch<ExtBackgroundEffectManagerV1, ()>,
    D: Dispatch<ExtBackgroundEffectSurfaceV1, BackgroundEffectSurfaceUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        manager: &ExtBackgroundEffectManagerV1,
        request: ManagerRequest,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ManagerRequest::GetBackgroundEffect { id, surface } => {
                let already_taken = with_states(&surface, |states| {
                    states
                        .data_map
                        .insert_if_missing_threadsafe(BackgroundEffectSurfaceData::new);
                    let data = states.data_map.get::<BackgroundEffectSurfaceData>().unwrap();
                    let already = data.is_resource_attached();
                    if !already {
                        data.set_is_resource_attached(true);
                    }
                    already
                });

                if already_taken {
                    manager.post_error(
                        ManagerError::BackgroundEffectExists,
                        "wl_surface already has a background effect object attached",
                    );
                } else {
                    data_init.init(id, BackgroundEffectSurfaceUserData::new(surface));
                }
            }
            ManagerRequest::Destroy => {}
            _ => {}
        }
    }
}

// Dispatch for ext_background_effect_surface_v1
impl<D> Dispatch<ExtBackgroundEffectSurfaceV1, BackgroundEffectSurfaceUserData, D> for BackgroundEffectState
where
    D: Dispatch<ExtBackgroundEffectSurfaceV1, BackgroundEffectSurfaceUserData>,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        obj: &ExtBackgroundEffectSurfaceV1,
        request: SurfaceRequest,
        data: &BackgroundEffectSurfaceUserData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            SurfaceRequest::SetBlurRegion { region } => {
                let Some(surface) = data.wl_surface() else {
                    obj.post_error(SurfaceError::SurfaceDestroyed, "wl_surface was destroyed");
                    return;
                };

                with_states(&surface, |states| {
                    let mut cached = states.cached_state.get::<BackgroundEffectSurfaceCachedState>();
                    let pending = cached.pending();
                    pending.blur_region =
                        region.map(|r| crate::wayland::compositor::get_region_attributes(&r));
                });
            }
            SurfaceRequest::Destroy => {
                let Some(surface) = data.wl_surface() else {
                    obj.post_error(SurfaceError::SurfaceDestroyed, "wl_surface was destroyed");
                    return;
                };

                with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<BackgroundEffectSurfaceData>()
                        .unwrap()
                        .set_is_resource_attached(false);
                    let mut cached = states.cached_state.get::<BackgroundEffectSurfaceCachedState>();
                    cached.pending().blur_region = None;
                });
            }
            _ => {}
        }
    }

    fn destroyed(
        _state: &mut D,
        _client_id: wayland_server::backend::ClientId,
        _object: &ExtBackgroundEffectSurfaceV1,
        _data: &BackgroundEffectSurfaceUserData,
    ) {
        // No-op: cleanup is handled by double-buffering and surface destruction
    }
}
