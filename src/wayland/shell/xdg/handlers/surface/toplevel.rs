use std::sync::atomic::Ordering;

use crate::{
    utils::Serial,
    wayland::{compositor, shell::xdg::XdgToplevelSurfaceData},
};

use wayland_protocols::xdg::shell::server::xdg_toplevel::{self, XdgToplevel};

use wayland_server::{
    backend::ClientId, protocol::wl_surface, DataInit, Dispatch, DisplayHandle, Resource, WEnum,
};

use super::{
    SurfaceCachedState, ToplevelConfigure, XdgShellHandler, XdgShellState, XdgShellSurfaceUserData,
    XdgSurfaceUserData, XdgToplevelSurfaceRoleAttributes,
};

impl<D> Dispatch<XdgToplevel, XdgShellSurfaceUserData, D> for XdgShellState
where
    D: Dispatch<XdgToplevel, XdgShellSurfaceUserData>,
    D: XdgShellHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        toplevel: &XdgToplevel,
        request: xdg_toplevel::Request,
        data: &XdgShellSurfaceUserData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_toplevel::Request::Destroy => {
                if let Some(surface_data) = data.xdg_surface.data::<XdgSurfaceUserData>() {
                    surface_data.has_active_role.store(false, Ordering::Release);
                }
            }
            xdg_toplevel::Request::SetParent { parent } => {
                let parent_surface = parent.map(|toplevel_surface_parent| {
                    toplevel_surface_parent
                        .data::<XdgShellSurfaceUserData>()
                        .unwrap()
                        .wl_surface
                        .clone()
                });

                // Parent is not double buffered, we can set it directly
                set_parent(toplevel, parent_surface);
            }
            xdg_toplevel::Request::SetTitle { title } => {
                // Title is not double buffered, we can set it directly
                let changed = with_surface_toplevel_role_data(toplevel, |role| {
                    if role.title.as_ref() != Some(&title) {
                        role.title = Some(title);
                        true
                    } else {
                        false
                    }
                });

                if changed {
                    let handle = make_toplevel_handle(toplevel);
                    XdgShellHandler::title_changed(state, handle);
                }
            }
            xdg_toplevel::Request::SetAppId { app_id } => {
                // AppId is not double buffered, we can set it directly
                let changed = with_surface_toplevel_role_data(toplevel, |role| {
                    if role.app_id.as_ref() != Some(&app_id) {
                        role.app_id = Some(app_id);
                        true
                    } else {
                        false
                    }
                });

                if changed {
                    let handle = make_toplevel_handle(toplevel);
                    XdgShellHandler::app_id_changed(state, handle);
                }
            }
            xdg_toplevel::Request::ShowWindowMenu { seat, serial, x, y } => {
                // This has to be handled by the compositor
                let handle = make_toplevel_handle(toplevel);
                let serial = Serial::from(serial);

                XdgShellHandler::show_window_menu(state, handle, seat, serial, (x, y).into());
            }
            xdg_toplevel::Request::Move { seat, serial } => {
                // This has to be handled by the compositor
                let handle = make_toplevel_handle(toplevel);
                let serial = Serial::from(serial);

                XdgShellHandler::move_request(state, handle, seat, serial);
            }
            xdg_toplevel::Request::Resize { seat, serial, edges } => {
                if let WEnum::Value(edges) = edges {
                    // This has to be handled by the compositor
                    let handle = make_toplevel_handle(toplevel);
                    let serial = Serial::from(serial);

                    XdgShellHandler::resize_request(state, handle, seat, serial, edges);
                }
            }
            xdg_toplevel::Request::SetMaxSize { width, height } => {
                with_toplevel_pending_state(data, |toplevel_data| {
                    toplevel_data.max_size = (width, height).into();
                });
            }
            xdg_toplevel::Request::SetMinSize { width, height } => {
                with_toplevel_pending_state(data, |toplevel_data| {
                    toplevel_data.min_size = (width, height).into();
                });
            }
            xdg_toplevel::Request::SetMaximized => {
                let handle = make_toplevel_handle(toplevel);
                XdgShellHandler::maximize_request(state, handle);
            }
            xdg_toplevel::Request::UnsetMaximized => {
                let handle = make_toplevel_handle(toplevel);
                XdgShellHandler::unmaximize_request(state, handle);
            }
            xdg_toplevel::Request::SetFullscreen { output } => {
                let handle = make_toplevel_handle(toplevel);
                XdgShellHandler::fullscreen_request(state, handle, output);
            }
            xdg_toplevel::Request::UnsetFullscreen => {
                let handle = make_toplevel_handle(toplevel);
                XdgShellHandler::unfullscreen_request(state, handle);
            }
            xdg_toplevel::Request::SetMinimized => {
                // This has to be handled by the compositor, may not be
                // supported and just ignored
                let handle = make_toplevel_handle(toplevel);
                XdgShellHandler::minimize_request(state, handle);
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut D,
        _client_id: ClientId,
        xdg_toplevel: &XdgToplevel,
        data: &XdgShellSurfaceUserData,
    ) {
        data.alive_tracker.destroy_notify();
        data.decoration.lock().unwrap().take();

        if let Some(index) = state
            .xdg_shell_state()
            .known_toplevels
            .iter()
            .position(|top| top.shell_surface.id() == xdg_toplevel.id())
        {
            let toplevel = state.xdg_shell_state().known_toplevels.remove(index);
            let surface = toplevel.wl_surface().clone();
            XdgShellHandler::toplevel_destroyed(state, toplevel);
            compositor::with_states(&surface, |states| {
                *states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap() = Default::default();
                *states.cached_state.pending::<SurfaceCachedState>() = Default::default();
                *states.cached_state.current::<SurfaceCachedState>() = Default::default();
            })
        }
    }
}

// Utility functions allowing to factor out a lot of the upcoming logic
fn with_surface_toplevel_role_data<F, T>(toplevel: &xdg_toplevel::XdgToplevel, f: F) -> T
where
    F: FnOnce(&mut XdgToplevelSurfaceRoleAttributes) -> T,
{
    let data = toplevel.data::<XdgShellSurfaceUserData>().unwrap();
    compositor::with_states(&data.wl_surface, |states| {
        f(&mut states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .unwrap()
            .lock()
            .unwrap())
    })
}

pub(super) fn make_toplevel_handle(
    resource: &xdg_toplevel::XdgToplevel,
) -> crate::wayland::shell::xdg::ToplevelSurface {
    let data = resource.data::<XdgShellSurfaceUserData>().unwrap();
    crate::wayland::shell::xdg::ToplevelSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: resource.clone(),
    }
}

pub fn get_parent(toplevel: &xdg_toplevel::XdgToplevel) -> Option<wl_surface::WlSurface> {
    with_surface_toplevel_role_data(toplevel, |data| data.parent.clone())
}

/// Sets the parent of the specified toplevel surface.
///
/// The parent must be a toplevel surface.
///
/// The parent of a surface is not double buffered and therefore may be set directly.
///
/// If the parent is `None`, the parent-child relationship is removed.
pub fn set_parent(toplevel: &xdg_toplevel::XdgToplevel, parent: Option<wl_surface::WlSurface>) {
    with_surface_toplevel_role_data(toplevel, |data| {
        data.parent = parent;
    });
}

fn with_toplevel_pending_state<F, T>(data: &XdgShellSurfaceUserData, f: F) -> T
where
    F: FnOnce(&mut SurfaceCachedState) -> T,
{
    compositor::with_states(&data.wl_surface, |states| {
        f(&mut states.cached_state.pending::<SurfaceCachedState>())
    })
}

pub fn send_toplevel_configure(
    resource: &xdg_toplevel::XdgToplevel,
    configure: ToplevelConfigure,
    send_bounds: bool,
    send_capabilities: bool,
) {
    let data = resource.data::<XdgShellSurfaceUserData>().unwrap();
    let (width, height) = configure.state.size.unwrap_or_default().into();
    // convert the Vec<State> (which is really a Vec<u32>) into Vec<u8>
    let states = {
        let mut states: Vec<xdg_toplevel::State> =
            configure.state.states.into_filtered_states(resource.version());
        let ptr = states.as_mut_ptr();
        let len = states.len();
        let cap = states.capacity();
        ::std::mem::forget(states);
        unsafe { Vec::from_raw_parts(ptr as *mut u8, len * 4, cap * 4) }
    };
    let serial = configure.serial;

    // send bounds if requested
    if send_bounds && resource.version() >= xdg_toplevel::EVT_CONFIGURE_BOUNDS_SINCE {
        let bounds = configure.state.bounds.unwrap_or_default();
        resource.configure_bounds(bounds.w, bounds.h);
    }

    // send the capabilities if requested
    if send_capabilities && resource.version() >= xdg_toplevel::EVT_WM_CAPABILITIES_SINCE {
        let mut capabilities = configure
            .state
            .capabilities
            .capabilities()
            .copied()
            .collect::<Vec<_>>();
        let capabilities = {
            let ptr = capabilities.as_mut_ptr();
            let len = capabilities.len();
            let cap = capabilities.capacity();
            ::std::mem::forget(capabilities);
            unsafe { Vec::from_raw_parts(ptr as *mut u8, len * 4, cap * 4) }
        };
        resource.wm_capabilities(capabilities);
    }

    // Send the toplevel configure
    resource.configure(width, height, states);

    // Send the base xdg_surface configure event to mark
    // The configure as finished
    data.xdg_surface.configure(serial.into());
}
