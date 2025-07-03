use wayland_protocols::wp::tearing_control::v1::server::{
    wp_tearing_control_manager_v1::{self, WpTearingControlManagerV1},
    wp_tearing_control_v1::{self, WpTearingControlV1},
};
use wayland_server::{
    backend::ClientId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use super::{
    TearingControlState, TearingControlSurfaceCachedState, TearingControlSurfaceData, TearingControlUserData,
};
use crate::wayland::compositor;

impl<D> GlobalDispatch<WpTearingControlManagerV1, (), D> for TearingControlState
where
    D: GlobalDispatch<WpTearingControlManagerV1, ()>,
    D: Dispatch<WpTearingControlManagerV1, ()>,
    D: Dispatch<WpTearingControlV1, TearingControlUserData>,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WpTearingControlManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<WpTearingControlManagerV1, (), D> for TearingControlState
where
    D: Dispatch<WpTearingControlManagerV1, ()>,
    D: Dispatch<WpTearingControlV1, TearingControlUserData>,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _: &Client,
        manager: &WpTearingControlManagerV1,
        request: wp_tearing_control_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_tearing_control_manager_v1::Request::GetTearingControl { id, surface } => {
                let already_taken = compositor::with_states(&surface, |states| {
                    states
                        .data_map
                        .insert_if_missing_threadsafe(TearingControlSurfaceData::new);
                    let data = states.data_map.get::<TearingControlSurfaceData>().unwrap();

                    let already_taken = data.is_resource_attached();

                    if !already_taken {
                        data.set_is_resource_attached(true);
                    }

                    already_taken
                });

                if already_taken {
                    manager.post_error(
                        wp_tearing_control_manager_v1::Error::TearingControlExists,
                        "WlSurface already has WpTearingControlV1 attached",
                    )
                } else {
                    data_init.init(id, TearingControlUserData::new(surface));
                }
            }

            wp_tearing_control_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<WpTearingControlV1, TearingControlUserData, D> for TearingControlState
where
    D: Dispatch<WpTearingControlV1, TearingControlUserData>,
{
    fn request(
        _state: &mut D,
        _: &Client,
        _: &WpTearingControlV1,
        request: wp_tearing_control_v1::Request,
        data: &TearingControlUserData,
        _dh: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_tearing_control_v1::Request::SetPresentationHint { hint } => {
                let wayland_server::WEnum::Value(hint) = hint else {
                    return;
                };
                let surface = data.wl_surface();

                compositor::with_states(&surface, |states| {
                    states
                        .cached_state
                        .pending::<TearingControlSurfaceCachedState>()
                        .presentation_hint = hint;
                })
            }
            // Switch back to default PresentationHint.
            // This is equivalent to setting the hint to Vsync,
            // including double buffering semantics.
            wp_tearing_control_v1::Request::Destroy => {
                let surface = data.wl_surface();

                compositor::with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<TearingControlSurfaceData>()
                        .unwrap()
                        .set_is_resource_attached(false);

                    states
                        .cached_state
                        .pending::<TearingControlSurfaceCachedState>()
                        .presentation_hint = wp_tearing_control_v1::PresentationHint::Vsync;
                });
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _client: ClientId,
        _object: &WpTearingControlV1,
        _data: &TearingControlUserData,
    ) {
        // Nothing to do here, graceful Destroy is already handled with double buffering
        // and in case of client close WlSurface destroyed handler will clean up the data anyway,
        // so there is no point in queuing new update
    }
}
