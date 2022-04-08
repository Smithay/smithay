use std::{convert::TryFrom, ops::Deref, sync::Mutex};

use wayland_protocols::wlr::unstable::layer_shell::v1::server::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use wayland_server::{protocol::wl_surface, DispatchData, Filter, Main};

use crate::wayland::{compositor, shell::wlr_layer::Layer, Serial};

use super::{
    Anchor, KeyboardInteractivity, LayerShellRequest, LayerSurfaceAttributes, LayerSurfaceCachedState,
    Margins, ShellUserData,
};

use super::LAYER_SURFACE_ROLE;

pub(super) fn layer_shell_implementation(
    shell: Main<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    request: zwlr_layer_shell_v1::Request,
    dispatch_data: DispatchData<'_>,
) {
    let data = shell.as_ref().user_data().get::<ShellUserData>().unwrap();
    match request {
        zwlr_layer_shell_v1::Request::GetLayerSurface {
            id,
            surface,
            output,
            layer,
            namespace,
        } => {
            let layer = match Layer::try_from(layer) {
                Ok(layer) => layer,
                Err((err, msg)) => {
                    id.as_ref().post_error(err as u32, msg);
                    return;
                }
            };

            if compositor::give_role(&surface, LAYER_SURFACE_ROLE).is_err() {
                shell.as_ref().post_error(
                    zwlr_layer_shell_v1::Error::Role as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }

            compositor::with_states(&surface, |states| {
                states.data_map.insert_if_missing_threadsafe(|| {
                    Mutex::new(LayerSurfaceAttributes::new(id.deref().clone()))
                });

                states.cached_state.pending::<LayerSurfaceCachedState>().layer = layer;
            })
            .unwrap();

            compositor::add_commit_hook(&surface, |surface| {
                compositor::with_states(surface, |states| {
                    let mut guard = states
                        .data_map
                        .get::<Mutex<LayerSurfaceAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap();

                    let pending = states.cached_state.pending::<LayerSurfaceCachedState>();

                    if pending.size.w == 0 && !pending.anchor.anchored_horizontally() {
                        guard.surface.as_ref().post_error(
                            zwlr_layer_surface_v1::Error::InvalidSize as u32,
                            "width 0 requested without setting left and right anchors".into(),
                        );
                        return;
                    }

                    if pending.size.h == 0 && !pending.anchor.anchored_vertically() {
                        guard.surface.as_ref().post_error(
                            zwlr_layer_surface_v1::Error::InvalidSize as u32,
                            "height 0 requested without setting top and bottom anchors".into(),
                        );
                        return;
                    }

                    if let Some(state) = guard.last_acked.clone() {
                        guard.current = state;
                    }
                })
                .unwrap();
            });

            id.quick_assign(|surface, req, dispatch_data| {
                layer_surface_implementation(surface.deref().clone(), req, dispatch_data)
            });

            id.assign_destructor(Filter::new(
                |layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, _, _| {
                    let data = layer_surface
                        .as_ref()
                        .user_data()
                        .get::<LayerSurfaceUserData>()
                        .unwrap();

                    // remove this surface from the known ones (as well as any leftover dead surface)
                    data.shell_data
                        .shell_state
                        .lock()
                        .unwrap()
                        .known_layers
                        .retain(|other| other.alive());
                },
            ));

            id.as_ref().user_data().set(|| LayerSurfaceUserData {
                shell_data: data.clone(),
                wl_surface: surface.clone(),
            });

            let handle = super::LayerSurface {
                wl_surface: surface,
                shell_surface: id.deref().clone(),
            };

            data.shell_state.lock().unwrap().known_layers.push(handle.clone());

            let mut user_impl = data.user_impl.borrow_mut();
            (*user_impl)(
                LayerShellRequest::NewLayerSurface {
                    surface: handle,
                    output,
                    layer,
                    namespace,
                },
                dispatch_data,
            );
        }
        zwlr_layer_shell_v1::Request::Destroy => {
            // Handled by destructor
        }
        _ => {}
    }
}

fn layer_surface_implementation(
    layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    request: zwlr_layer_surface_v1::Request,
    dispatch_data: DispatchData<'_>,
) {
    match request {
        zwlr_layer_surface_v1::Request::SetSize { width, height } => {
            with_surface_pending_state(&layer_surface, |data| {
                data.size = (width as i32, height as i32).into();
            });
        }
        zwlr_layer_surface_v1::Request::SetAnchor { anchor } => {
            match Anchor::try_from(anchor) {
                Ok(anchor) => {
                    with_surface_pending_state(&layer_surface, |data| {
                        data.anchor = Anchor::from_bits(anchor.bits()).unwrap_or_default();
                    });
                }
                Err((err, msg)) => {
                    layer_surface.as_ref().post_error(err as u32, msg);
                }
            };
        }
        zwlr_layer_surface_v1::Request::SetExclusiveZone { zone } => {
            with_surface_pending_state(&layer_surface, |data| {
                data.exclusive_zone = zone.into();
            });
        }
        zwlr_layer_surface_v1::Request::SetMargin {
            top,
            right,
            bottom,
            left,
        } => {
            with_surface_pending_state(&layer_surface, |data| {
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
                    with_surface_pending_state(&layer_surface, |data| {
                        data.keyboard_interactivity = keyboard_interactivity;
                    });
                }
                Err((err, msg)) => {
                    layer_surface.as_ref().post_error(err as u32, msg);
                }
            };
        }
        zwlr_layer_surface_v1::Request::SetLayer { layer } => {
            match Layer::try_from(layer) {
                Ok(layer) => {
                    with_surface_pending_state(&layer_surface, |data| {
                        data.layer = layer;
                    });
                }
                Err((err, msg)) => {
                    layer_surface.as_ref().post_error(err as u32, msg);
                }
            };
        }
        zwlr_layer_surface_v1::Request::GetPopup { popup } => {
            let data = layer_surface
                .as_ref()
                .user_data()
                .get::<LayerSurfaceUserData>()
                .unwrap();

            let parent_surface = data.wl_surface.clone();

            let data = popup
                .as_ref()
                .user_data()
                .get::<crate::wayland::shell::xdg::xdg_handlers::ShellSurfaceUserData>()
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
            let data = layer_surface
                .as_ref()
                .user_data()
                .get::<LayerSurfaceUserData>()
                .unwrap();

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
                    layer_surface.as_ref().post_error(
                        zwlr_layer_surface_v1::Error::InvalidSurfaceState as u32,
                        format!("wrong configure serial: {}", <u32>::from(serial)),
                    );
                    return;
                }
            };

            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (*user_impl)(
                LayerShellRequest::AckConfigure {
                    surface: data.wl_surface.clone(),
                    configure,
                },
                dispatch_data,
            );
        }
        _ => {}
    }
}

struct LayerSurfaceUserData {
    shell_data: ShellUserData,
    wl_surface: wl_surface::WlSurface,
}

fn with_surface_pending_state<F, T>(layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, f: F) -> T
where
    F: FnOnce(&mut LayerSurfaceCachedState) -> T,
{
    let data = layer_surface
        .as_ref()
        .user_data()
        .get::<LayerSurfaceUserData>()
        .unwrap();
    compositor::with_states(&data.wl_surface, |states| {
        f(&mut *states.cached_state.pending::<LayerSurfaceCachedState>())
    })
    .unwrap()
}
