use crate::wayland::background_effect::{
    BackgroundEffectState, BackgroundEffectSurfaceData, ExtBackgroundEffectHandler,
};
use crate::wayland::compositor;
use crate::wayland::{
    background_effect::{BackgroundEffectSurfaceCachedState, BackgroundEffectSurfaceUserData},
    compositor::with_states,
};
use wayland_protocols::ext::background_effect::v1::server::{
    ext_background_effect_manager_v1::{
        Error as ManagerError, ExtBackgroundEffectManagerV1, Request as ManagerRequest,
    },
    ext_background_effect_surface_v1::{
        Error as SurfaceError, ExtBackgroundEffectSurfaceV1, Request as SurfaceRequest,
    },
};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

impl<D: ExtBackgroundEffectHandler> GlobalDispatch<ExtBackgroundEffectManagerV1, (), D>
    for BackgroundEffectState
{
    fn bind(
        state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtBackgroundEffectManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        let manager = data_init.init(resource, ());
        manager.capabilities(state.capabilities());
    }
}

impl<D: ExtBackgroundEffectHandler> Dispatch<ExtBackgroundEffectManagerV1, (), D> for BackgroundEffectState {
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

impl<D: ExtBackgroundEffectHandler> Dispatch<ExtBackgroundEffectSurfaceV1, BackgroundEffectSurfaceUserData, D>
    for BackgroundEffectState
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
                    pending.blur_region = region.map(|r| compositor::get_region_attributes(&r));
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
