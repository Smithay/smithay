use std::sync::Mutex;

use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_shell_v1::{self, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1;
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;
use wayland_server::protocol::wl_surface;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, Resource};

use crate::utils::{
    alive_tracker::{AliveTracker, IsAlive},
    Serial,
};
use crate::wayland::shell::xdg::XdgPopupSurfaceData;
use crate::wayland::{compositor, shell::wlr_layer::Layer};

use super::{
    Anchor, KeyboardInteractivity, LayerSurfaceAttributes, LayerSurfaceCachedState, LayerSurfaceData,
    Margins, WlrLayerShellGlobalData, WlrLayerShellHandler, WlrLayerShellState,
};

use super::LAYER_SURFACE_ROLE;

/*
 * layer_shell
 */

impl<D> GlobalDispatch<ZwlrLayerShellV1, WlrLayerShellGlobalData, D> for WlrLayerShellState
where
    D: GlobalDispatch<ZwlrLayerShellV1, WlrLayerShellGlobalData>,
    D: Dispatch<ZwlrLayerShellV1, ()>,
    D: Dispatch<ZwlrLayerSurfaceV1, WlrLayerSurfaceUserData>,
    D: WlrLayerShellHandler,
    D: 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: wayland_server::New<ZwlrLayerShellV1>,
        _global_data: &WlrLayerShellGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &WlrLayerShellGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ZwlrLayerShellV1, (), D> for WlrLayerShellState
where
    D: Dispatch<ZwlrLayerShellV1, ()>,
    D: Dispatch<ZwlrLayerSurfaceV1, WlrLayerSurfaceUserData>,
    D: WlrLayerShellHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        shell: &ZwlrLayerShellV1,
        request: zwlr_layer_shell_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
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
                            zwlr_layer_shell_v1::Error::InvalidLayer,
                            format!("invalid layer: {:?}", layer),
                        );
                        return;
                    }
                };

                if compositor::give_role(&wl_surface, LAYER_SURFACE_ROLE).is_err() {
                    shell.post_error(zwlr_layer_shell_v1::Error::Role, "Surface already has a role.");
                    return;
                }

                let id = data_init.init(
                    id,
                    WlrLayerSurfaceUserData {
                        shell_data: state.shell_state().clone(),
                        wl_surface: wl_surface.clone(),
                        alive_tracker: Default::default(),
                    },
                );

                let initial = compositor::with_states(&wl_surface, |states| {
                    let inserted = states
                        .data_map
                        .insert_if_missing_threadsafe(|| Mutex::new(LayerSurfaceAttributes::new(id.clone())));

                    if !inserted {
                        let mut attributes = states
                            .data_map
                            .get::<Mutex<LayerSurfaceAttributes>>()
                            .unwrap()
                            .lock()
                            .unwrap();
                        attributes.surface = id.clone();
                    }

                    states.cached_state.pending::<LayerSurfaceCachedState>().layer = layer;

                    inserted
                });

                if initial {
                    compositor::add_pre_commit_hook::<D, _>(&wl_surface, |_state, _dh, surface| {
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
                                    zwlr_layer_surface_v1::Error::InvalidSize,
                                    "width 0 requested without setting left and right anchors",
                                );
                                return;
                            }

                            if pending.size.h == 0 && !pending.anchor.anchored_vertically() {
                                guard.surface.post_error(
                                    zwlr_layer_surface_v1::Error::InvalidSize,
                                    "height 0 requested without setting top and bottom anchors",
                                );
                                return;
                            }

                            if let Some(state) = guard.last_acked.clone() {
                                guard.current = state;
                            }
                        });
                    });
                }

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

                WlrLayerShellHandler::new_layer_surface(state, handle, output, layer, namespace);
            }
            zwlr_layer_shell_v1::Request::Destroy => {
                // Handled by destructor
            }
            _ => {}
        }
    }
}

/*
 * layer_surface
 */

/// User data for wlr layer surface
#[derive(Debug)]
pub struct WlrLayerSurfaceUserData {
    shell_data: WlrLayerShellState,
    wl_surface: wl_surface::WlSurface,
    alive_tracker: AliveTracker,
}

impl IsAlive for ZwlrLayerSurfaceV1 {
    fn alive(&self) -> bool {
        let data: &WlrLayerSurfaceUserData = self.data().unwrap();
        data.alive_tracker.alive()
    }
}

impl<D> Dispatch<ZwlrLayerSurfaceV1, WlrLayerSurfaceUserData, D> for WlrLayerShellState
where
    D: Dispatch<ZwlrLayerSurfaceV1, WlrLayerSurfaceUserData>,
    D: WlrLayerShellHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        layer_surface: &ZwlrLayerSurfaceV1,
        request: zwlr_layer_surface_v1::Request,
        data: &WlrLayerSurfaceUserData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
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
                        layer_surface.post_error(err, msg);
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
                        layer_surface.post_error(err, msg);
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
                        layer_surface.post_error(err, msg);
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
                        .get::<XdgPopupSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .parent = Some(parent_surface);
                });

                WlrLayerShellHandler::new_popup(
                    state,
                    make_surface_handle(layer_surface),
                    crate::wayland::shell::xdg::handlers::make_popup_handle(&popup),
                );
            }
            zwlr_layer_surface_v1::Request::AckConfigure { serial } => {
                let serial = Serial::from(serial);
                let surface = &data.wl_surface;

                let found_configure = compositor::with_states(surface, |states| {
                    states
                        .data_map
                        .get::<LayerSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .ack_configure(serial)
                });

                let configure = match found_configure {
                    Some(configure) => configure,
                    None => {
                        layer_surface.post_error(
                            zwlr_layer_surface_v1::Error::InvalidSurfaceState,
                            format!("wrong configure serial: {}", <u32>::from(serial)),
                        );
                        return;
                    }
                };

                WlrLayerShellHandler::ack_configure(state, data.wl_surface.clone(), configure);
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut D,
        _client_id: wayland_server::backend::ClientId,
        layer_surface: &ZwlrLayerSurfaceV1,
        data: &WlrLayerSurfaceUserData,
    ) {
        data.alive_tracker.destroy_notify();

        // remove this surface from the known ones (as well as any leftover dead surface)
        let mut layers = data.shell_data.known_layers.lock().unwrap();
        if let Some(index) = layers
            .iter()
            .position(|layer| layer.shell_surface.id() == layer_surface.id())
        {
            let layer = layers.remove(index);
            drop(layers);
            let surface = layer.wl_surface().clone();
            WlrLayerShellHandler::layer_destroyed(state, layer);
            compositor::with_states(&surface, |states| {
                let mut attributes = states
                    .data_map
                    .get::<Mutex<LayerSurfaceAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap();
                attributes.reset();
                *states.cached_state.pending::<LayerSurfaceCachedState>() = Default::default();
                *states.cached_state.current::<LayerSurfaceCachedState>() = Default::default();
            });
        }
    }
}

fn with_surface_pending_state<F, T>(layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, f: F) -> T
where
    F: FnOnce(&mut LayerSurfaceCachedState) -> T,
{
    let data = layer_surface.data::<WlrLayerSurfaceUserData>().unwrap();
    compositor::with_states(&data.wl_surface, |states| {
        f(&mut states.cached_state.pending::<LayerSurfaceCachedState>())
    })
}

pub fn make_surface_handle(
    resource: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
) -> crate::wayland::shell::wlr_layer::LayerSurface {
    let data = resource.data::<WlrLayerSurfaceUserData>().unwrap();
    crate::wayland::shell::wlr_layer::LayerSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: resource.clone(),
    }
}
