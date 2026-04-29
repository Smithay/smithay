use wayland_protocols::wp::alpha_modifier::v1::server::{
    wp_alpha_modifier_surface_v1::{self, WpAlphaModifierSurfaceV1},
    wp_alpha_modifier_v1::{self, WpAlphaModifierV1},
};

use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, backend::ClientId,
};

use super::{AlphaModifierSurfaceCachedState, AlphaModifierSurfaceData, AlphaModifierSurfaceUserData};
use crate::wayland::{GlobalData, compositor};

impl<D> GlobalDispatch<WpAlphaModifierV1, D> for GlobalData
where
    D: 'static,
{
    fn bind(
        &self,
        _state: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WpAlphaModifierV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }
}

impl<D> Dispatch<WpAlphaModifierV1, D> for GlobalData
where
    D: 'static,
{
    fn request(
        &self,
        _state: &mut D,
        _: &Client,
        manager: &WpAlphaModifierV1,
        request: wp_alpha_modifier_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_alpha_modifier_v1::Request::GetSurface { id, surface } => {
                let already_taken = compositor::with_states(&surface, |states| {
                    states
                        .data_map
                        .insert_if_missing_threadsafe(AlphaModifierSurfaceData::new);
                    let data = states.data_map.get::<AlphaModifierSurfaceData>().unwrap();

                    let already_taken = data.is_resource_attached();

                    if !already_taken {
                        data.set_is_resource_attached(true);
                    }

                    already_taken
                });

                if already_taken {
                    manager.post_error(
                        wp_alpha_modifier_v1::Error::AlreadyConstructed,
                        "wl_surface already has a alpha modifier object attached",
                    )
                } else {
                    data_init.init(id, AlphaModifierSurfaceUserData::new(surface));
                }
            }

            wp_alpha_modifier_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<WpAlphaModifierSurfaceV1, D> for AlphaModifierSurfaceUserData {
    fn request(
        &self,
        _state: &mut D,
        _: &Client,
        obj: &WpAlphaModifierSurfaceV1,
        request: wp_alpha_modifier_surface_v1::Request,
        _dh: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_alpha_modifier_surface_v1::Request::SetMultiplier { factor } => {
                let Some(surface) = self.wl_surface() else {
                    obj.post_error(
                        wp_alpha_modifier_surface_v1::Error::NoSurface,
                        "wl_surface was destroyed",
                    );
                    return;
                };

                compositor::with_states(&surface, |states| {
                    states
                        .cached_state
                        .get::<AlphaModifierSurfaceCachedState>()
                        .pending()
                        .multiplier = Some(factor);
                })
            }
            // Switch back to not specifying the alpha multiplier of this surface.
            wp_alpha_modifier_surface_v1::Request::Destroy => {
                let Some(surface) = self.wl_surface() else {
                    obj.post_error(
                        wp_alpha_modifier_surface_v1::Error::NoSurface,
                        "wl_surface was destroyed",
                    );
                    return;
                };

                compositor::with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<AlphaModifierSurfaceData>()
                        .unwrap()
                        .set_is_resource_attached(false);

                    states
                        .cached_state
                        .get::<AlphaModifierSurfaceCachedState>()
                        .pending()
                        .multiplier = None;
                });
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(&self, _state: &mut D, _client: ClientId, _object: &WpAlphaModifierSurfaceV1) {
        // Nothing to do here, graceful Destroy is already handled with double buffering
        // and in case of client close WlSurface destroyed handler will clean up the data anyway,
        // so there is no point in queuing new update
    }
}
