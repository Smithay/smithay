use std::{convert::TryFrom, sync::Mutex};

use wayland_protocols::wlr::unstable::layer_shell::v1::server::zwlr_layer_shell_v1::{
    self, ZwlrLayerShellV1,
};
use wayland_protocols::wlr::unstable::layer_shell::v1::server::zwlr_layer_surface_v1;
use wayland_protocols::wlr::unstable::layer_shell::v1::server::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;
use wayland_server::protocol::wl_surface;
use wayland_server::{
    DelegateDispatch, DelegateDispatchBase, DelegateGlobalDispatch, DelegateGlobalDispatchBase, Dispatch,
    GlobalDispatch, Resource,
};

use crate::wayland::{compositor, shell::wlr_layer::Layer, Serial};

use super::{
    Anchor, KeyboardInteractivity, LayerShellRequest, LayerSurfaceAttributes, LayerSurfaceCachedState,
    Margins, WlrLayerShellHandler, WlrLayerShellState,
};

use super::LAYER_SURFACE_ROLE;

impl DelegateGlobalDispatchBase<ZwlrLayerShellV1> for WlrLayerShellState {
    type GlobalData = ();
}

impl<D> DelegateGlobalDispatch<ZwlrLayerShellV1, D> for WlrLayerShellState
where
    D: GlobalDispatch<ZwlrLayerShellV1, GlobalData = ()>,
    D: Dispatch<ZwlrLayerShellV1, UserData = ()>,
    D: Dispatch<ZwlrLayerSurfaceV1, UserData = WlrLayerSurfaceUserData>,
    D: WlrLayerShellHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &mut wayland_server::DisplayHandle<'_>,
        _client: &wayland_server::Client,
        resource: wayland_server::New<ZwlrLayerShellV1>,
        _global_data: &Self::GlobalData,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl DelegateDispatchBase<ZwlrLayerShellV1> for WlrLayerShellState {
    type UserData = ();
}

impl<D> DelegateDispatch<ZwlrLayerShellV1, D> for WlrLayerShellState
where
    D: Dispatch<ZwlrLayerShellV1, UserData = ()>,
    D: Dispatch<ZwlrLayerSurfaceV1, UserData = WlrLayerSurfaceUserData>,
    D: WlrLayerShellHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        shell: &ZwlrLayerShellV1,
        request: zwlr_layer_shell_v1::Request,
        _data: &Self::UserData,
        dh: &mut wayland_server::DisplayHandle<'_>,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwlr_layer_shell_v1::Request::GetLayerSurface {
                id,
                surface: wl_surface,
                output,
                layer,
                namespace,
            } => {
                let layer: Layer = match layer.try_into() {
                    Ok(layer) => layer,
                    Err(layer) => {
                        shell.post_error(
                            dh,
                            zwlr_layer_shell_v1::Error::InvalidLayer,
                            format!("invalid layer: {:?}", layer),
                        );
                        return;
                    }
                };

                if compositor::give_role(&wl_surface, LAYER_SURFACE_ROLE).is_err() {
                    shell.post_error(
                        dh,
                        zwlr_layer_shell_v1::Error::Role,
                        "Surface already has a role.",
                    );
                    return;
                }

                let id = data_init.init(
                    id,
                    WlrLayerSurfaceUserData {
                        shell_data: state.shell_state().clone(),
                        wl_surface: wl_surface.clone(),
                    },
                );

                compositor::with_states(&wl_surface, |states| {
                    states
                        .data_map
                        .insert_if_missing_threadsafe(|| Mutex::new(LayerSurfaceAttributes::new(id.clone())));

                    states.cached_state.pending::<LayerSurfaceCachedState>().layer = layer;
                })
                .unwrap();

                compositor::add_pre_commit_hook(&wl_surface, |dh, surface| {
                    compositor::with_states(surface, |states| {
                        let mut guard = states
                            .data_map
                            .get::<Mutex<LayerSurfaceAttributes>>()
                            .unwrap()
                            .lock()
                            .unwrap();

                        let pending = states.cached_state.pending::<LayerSurfaceCachedState>();

                        if pending.size.w == 0 && !pending.anchor.anchored_horizontally() {
                            guard.surface.post_error(
                                dh,
                                zwlr_layer_surface_v1::Error::InvalidSize,
                                "width 0 requested without setting left and right anchors",
                            );
                            return;
                        }

                        if pending.size.h == 0 && !pending.anchor.anchored_vertically() {
                            guard.surface.post_error(
                                dh,
                                zwlr_layer_surface_v1::Error::InvalidSize,
                                "height 0 requested without setting top and bottom anchors",
                            );
                            return;
                        }

                        if let Some(state) = guard.last_acked.clone() {
                            guard.current = state;
                        }
                    })
                    .unwrap();
                });

                let handle = super::LayerSurface {
                    wl_surface,
                    shell_surface: id,
                };

                state
                    .shell_state()
                    .known_layers
                    .lock()
                    .unwrap()
                    .push(handle.clone());

                WlrLayerShellHandler::request(
                    state,
                    LayerShellRequest::NewLayerSurface {
                        surface: handle,
                        output,
                        layer,
                        namespace,
                    },
                );
            }
            zwlr_layer_shell_v1::Request::Destroy => {
                // Handled by destructor
            }
            _ => {}
        }
    }
}

impl DelegateDispatchBase<ZwlrLayerSurfaceV1> for WlrLayerShellState {
    type UserData = WlrLayerSurfaceUserData;
}

impl<D> DelegateDispatch<ZwlrLayerSurfaceV1, D> for WlrLayerShellState
where
    D: Dispatch<ZwlrLayerSurfaceV1, UserData = WlrLayerSurfaceUserData>,
    D: WlrLayerShellHandler,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        layer_surface: &ZwlrLayerSurfaceV1,
        request: zwlr_layer_surface_v1::Request,
        data: &Self::UserData,
        dh: &mut wayland_server::DisplayHandle<'_>,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwlr_layer_surface_v1::Request::SetSize { width, height } => {
                with_surface_pending_state(layer_surface, |data| {
                    data.size = (width as i32, height as i32).into();
                });
            }
            zwlr_layer_surface_v1::Request::SetAnchor { anchor } => {
                match Anchor::try_from(anchor) {
                    Ok(anchor) => {
                        with_surface_pending_state(layer_surface, |data| {
                            data.anchor = Anchor::from_bits(anchor.bits()).unwrap_or_default();
                        });
                    }
                    Err((err, msg)) => {
                        layer_surface.post_error(dh, err, msg);
                    }
                };
            }
            zwlr_layer_surface_v1::Request::SetExclusiveZone { zone } => {
                with_surface_pending_state(layer_surface, |data| {
                    data.exclusive_zone = zone.into();
                });
            }
            zwlr_layer_surface_v1::Request::SetMargin {
                top,
                right,
                bottom,
                left,
            } => {
                with_surface_pending_state(layer_surface, |data| {
                    data.margin = Margins {
                        top,
                        right,
                        bottom,
                        left,
                    };
                });
            }
            zwlr_layer_surface_v1::Request::SetKeyboardInteractivity {
                keyboard_interactivity,
            } => {
                match KeyboardInteractivity::try_from(keyboard_interactivity) {
                    Ok(keyboard_interactivity) => {
                        with_surface_pending_state(layer_surface, |data| {
                            data.keyboard_interactivity = keyboard_interactivity;
                        });
                    }
                    Err((err, msg)) => {
                        layer_surface.post_error(dh, err, msg);
                    }
                };
            }
            zwlr_layer_surface_v1::Request::SetLayer { layer } => {
                match Layer::try_from(layer) {
                    Ok(layer) => {
                        with_surface_pending_state(layer_surface, |data| {
                            data.layer = layer;
                        });
                    }
                    Err((err, msg)) => {
                        layer_surface.post_error(dh, err, msg);
                    }
                };
            }
            zwlr_layer_surface_v1::Request::GetPopup { popup } => {
                let parent_surface = data.wl_surface.clone();

                let data = popup
                    .data::<crate::wayland::shell::xdg::XdgShellSurfaceUserData>()
                    .unwrap();

                compositor::with_states(&data.wl_surface, move |states| {
                    states
                        .data_map
                        .get::<Mutex<crate::wayland::shell::xdg::XdgPopupSurfaceRoleAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .parent = Some(parent_surface);
                })
                .unwrap();
            }
            zwlr_layer_surface_v1::Request::AckConfigure { serial } => {
                let serial = Serial::from(serial);
                let surface = &data.wl_surface;

                let found_configure = compositor::with_states(surface, |states| {
                    states
                        .data_map
                        .get::<Mutex<LayerSurfaceAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .ack_configure(serial)
                })
                .unwrap();

                let configure = match found_configure {
                    Some(configure) => configure,
                    None => {
                        layer_surface.post_error(
                            dh,
                            zwlr_layer_surface_v1::Error::InvalidSurfaceState,
                            format!("wrong configure serial: {}", <u32>::from(serial)),
                        );
                        return;
                    }
                };

                WlrLayerShellHandler::request(
                    state,
                    LayerShellRequest::AckConfigure {
                        surface: data.wl_surface.clone(),
                        configure,
                    },
                );
            }
            _ => {}
        }
    }

    fn destroyed(
        _state: &mut D,
        _client_id: wayland_server::backend::ClientId,
        object_id: wayland_server::backend::ObjectId,
        data: &Self::UserData,
    ) {
        // remove this surface from the known ones (as well as any leftover dead surface)
        data.shell_data
            .known_layers
            .lock()
            .unwrap()
            .retain(|other| other.shell_surface.id() != object_id);
    }
}

/// User data for wlr layer surface
#[derive(Debug)]
pub struct WlrLayerSurfaceUserData {
    shell_data: WlrLayerShellState,
    wl_surface: wl_surface::WlSurface,
}

fn with_surface_pending_state<F, T>(layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, f: F) -> T
where
    F: FnOnce(&mut LayerSurfaceCachedState) -> T,
{
    let data = layer_surface.data::<WlrLayerSurfaceUserData>().unwrap();
    compositor::with_states(&data.wl_surface, |states| {
        f(&mut *states.cached_state.pending::<LayerSurfaceCachedState>())
    })
    .unwrap()
}
