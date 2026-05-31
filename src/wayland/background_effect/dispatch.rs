use crate::wayland::background_effect::{BackgroundEffectSurfaceData, ExtBackgroundEffectHandler};
use crate::wayland::compositor;
use crate::wayland::{
    Dispatch2, GlobalData, GlobalDispatch2,
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
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, New, Resource};

impl<D: ExtBackgroundEffectHandler> GlobalDispatch2<ExtBackgroundEffectManagerV1, D> for GlobalData
where
    D: Dispatch<ExtBackgroundEffectManagerV1, GlobalData>,
{
    fn bind(
        &self,
        state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtBackgroundEffectManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let manager = data_init.init(resource, GlobalData);
        manager.capabilities(state.capabilities());
    }
}

impl<D: ExtBackgroundEffectHandler> Dispatch2<ExtBackgroundEffectManagerV1, D> for GlobalData
where
    D: Dispatch<ExtBackgroundEffectSurfaceV1, BackgroundEffectSurfaceUserData>,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &Client,
        manager: &ExtBackgroundEffectManagerV1,
        request: ManagerRequest,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ManagerRequest::GetBackgroundEffect { id, surface } => {
                let already_taken = with_states(&surface, |states| {
                    let data = states
                        .data_map
                        .get_or_insert_threadsafe(BackgroundEffectSurfaceData::new);
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

impl<D: ExtBackgroundEffectHandler> Dispatch2<ExtBackgroundEffectSurfaceV1, D>
    for BackgroundEffectSurfaceUserData
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        obj: &ExtBackgroundEffectSurfaceV1,
        request: SurfaceRequest,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            SurfaceRequest::SetBlurRegion { region } => {
                let Some(surface) = self.wl_surface() else {
                    obj.post_error(SurfaceError::SurfaceDestroyed, "wl_surface was destroyed");
                    return;
                };

                let region = region.as_ref().map(compositor::get_region_attributes);

                with_states(&surface, |states| {
                    let mut cached = states.cached_state.get::<BackgroundEffectSurfaceCachedState>();
                    let pending = cached.pending();
                    pending.blur_region = region.clone();
                });

                if let Some(region) = region {
                    state.set_blur_region(surface, region);
                } else {
                    state.unset_blur_region(surface);
                }
            }
            SurfaceRequest::Destroy => {
                let Some(surface) = self.wl_surface() else {
                    // object is inert, but destroy is still allowed. We are done here.
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

                state.unset_blur_region(surface);
            }
            _ => {}
        }
    }

    fn destroyed(
        &self,
        _state: &mut D,
        _client_id: wayland_server::backend::ClientId,
        _object: &ExtBackgroundEffectSurfaceV1,
    ) {
        // No-op: cleanup is handled by double-buffering and surface destruction
    }
}
