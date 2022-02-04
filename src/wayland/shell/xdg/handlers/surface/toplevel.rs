use std::sync::{atomic::Ordering, Mutex};

use crate::wayland::{compositor, Serial};

use wayland_protocols::xdg_shell::server::xdg_toplevel::{self, XdgToplevel};

use wayland_server::{
    backend::{ClientId, ObjectId},
    protocol::wl_surface,
    DataInit, DelegateDispatch, DelegateDispatchBase, Dispatch, DisplayHandle, Resource, WEnum,
};

use super::{
    SurfaceCachedState, SurfaceKind, ToplevelConfigure, XdgRequest, XdgShellHandler, XdgShellState,
    XdgShellSurfaceUserData, XdgSurfaceUserData, XdgToplevelSurfaceRoleAttributes,
};

impl DelegateDispatchBase<XdgToplevel> for XdgShellState {
    type UserData = XdgShellSurfaceUserData;
}

impl<D> DelegateDispatch<XdgToplevel, D> for XdgShellState
where
    D: Dispatch<XdgToplevel, UserData = XdgShellSurfaceUserData>,
    D: XdgShellHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        toplevel: &XdgToplevel,
        request: xdg_toplevel::Request,
        data: &Self::UserData,
        dh: &mut DisplayHandle<'_>,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_toplevel::Request::Destroy => {
                // all it done by the destructor
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
                with_surface_toplevel_role_data(toplevel, |data| {
                    data.title = Some(title);
                });
            }
            xdg_toplevel::Request::SetAppId { app_id } => {
                // AppId is not double buffered, we can set it directly
                with_surface_toplevel_role_data(toplevel, |role| {
                    role.app_id = Some(app_id);
                });
            }
            xdg_toplevel::Request::ShowWindowMenu { seat, serial, x, y } => {
                // This has to be handled by the compositor
                let handle = make_toplevel_handle(toplevel);
                let serial = Serial::from(serial);

                XdgShellHandler::request(
                    state,
                    dh,
                    XdgRequest::ShowWindowMenu {
                        surface: handle,
                        seat,
                        serial,
                        location: (x, y).into(),
                    },
                );
            }
            xdg_toplevel::Request::Move { seat, serial } => {
                // This has to be handled by the compositor
                let handle = make_toplevel_handle(toplevel);
                let serial = Serial::from(serial);

                XdgShellHandler::request(
                    state,
                    dh,
                    XdgRequest::Move {
                        surface: handle,
                        seat,
                        serial,
                    },
                );
            }
            xdg_toplevel::Request::Resize { seat, serial, edges } => {
                if let WEnum::Value(edges) = edges {
                    // This has to be handled by the compositor
                    let handle = make_toplevel_handle(toplevel);
                    let serial = Serial::from(serial);

                    XdgShellHandler::request(
                        state,
                        dh,
                        XdgRequest::Resize {
                            surface: handle,
                            seat,
                            serial,
                            edges,
                        },
                    );
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
                XdgShellHandler::request(state, dh, XdgRequest::Maximize { surface: handle });
            }
            xdg_toplevel::Request::UnsetMaximized => {
                let handle = make_toplevel_handle(toplevel);
                XdgShellHandler::request(state, dh, XdgRequest::UnMaximize { surface: handle });
            }
            xdg_toplevel::Request::SetFullscreen { output } => {
                let handle = make_toplevel_handle(toplevel);
                XdgShellHandler::request(
                    state,
                    dh,
                    XdgRequest::Fullscreen {
                        surface: handle,
                        output,
                    },
                );
            }
            xdg_toplevel::Request::UnsetFullscreen => {
                let handle = make_toplevel_handle(toplevel);
                XdgShellHandler::request(state, dh, XdgRequest::UnFullscreen { surface: handle });
            }
            xdg_toplevel::Request::SetMinimized => {
                // This has to be handled by the compositor, may not be
                // supported and just ignored
                let handle = make_toplevel_handle(toplevel);
                XdgShellHandler::request(state, dh, XdgRequest::Minimize { surface: handle });
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(_state: &mut D, _client_id: ClientId, object_id: ObjectId, data: &Self::UserData) {
        if let Some(surface_data) = data.xdg_surface.data::<XdgSurfaceUserData>() {
            surface_data.has_active_role.store(false, Ordering::Release);
        }

        match &data.kind {
            SurfaceKind::Toplevel => {
                // remove this surface from the known ones (as well as any leftover dead surface)
                data.shell_data
                    .lock()
                    .unwrap()
                    .known_toplevels
                    .retain(|other| other.shell_surface.id() != object_id);
            }
            SurfaceKind::Popup => {
                // remove this surface from the known ones (as well as any leftover dead surface)
                data.shell_data
                    .lock()
                    .unwrap()
                    .known_popups
                    .retain(|other| other.shell_surface.id() != object_id);
            }
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
        f(&mut *states
            .data_map
            .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
            .unwrap()
            .lock()
            .unwrap())
    })
    .unwrap()
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
        f(&mut *states.cached_state.pending::<SurfaceCachedState>())
    })
    .unwrap()
}

pub fn send_toplevel_configure(
    dh: &mut DisplayHandle<'_>,
    resource: &xdg_toplevel::XdgToplevel,
    configure: ToplevelConfigure,
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

    // Send the toplevel configure
    resource.configure(dh, width, height, states);

    // Send the base xdg_surface configure event to mark
    // The configure as finished
    data.xdg_surface.configure(dh, serial.into());
}
