use std::sync::atomic::{AtomicBool, Ordering};
use std::{cell::RefCell, ops::Deref as _, sync::Mutex};

use crate::wayland::compositor;
use crate::wayland::shell::xdg::PopupState;
use crate::wayland::Serial;
use wayland_protocols::{
    unstable::xdg_shell::v6::server::{
        zxdg_popup_v6, zxdg_positioner_v6, zxdg_shell_v6, zxdg_surface_v6, zxdg_toplevel_v6,
    },
    xdg_shell::server::{xdg_positioner, xdg_toplevel},
};
use wayland_server::DispatchData;
use wayland_server::{protocol::wl_surface, Filter, Main};

use crate::utils::Rectangle;

use super::{
    make_shell_client_data, PopupConfigure, PopupKind, PositionerState, ShellClient, ShellClientData,
    ShellData, SurfaceCachedState, ToplevelConfigure, ToplevelKind, XdgPopupSurfaceRoleAttributes,
    XdgRequest, XdgToplevelSurfaceRoleAttributes,
};

static ZXDG_TOPLEVEL_ROLE: &str = "zxdg_toplevel";
static ZXDG_POPUP_ROLE: &str = "zxdg_toplevel";

pub(crate) fn implement_shell(
    shell: Main<zxdg_shell_v6::ZxdgShellV6>,
    shell_data: &ShellData,
    dispatch_data: DispatchData<'_>,
) -> zxdg_shell_v6::ZxdgShellV6 {
    shell.quick_assign(shell_implementation);
    shell.as_ref().user_data().set(|| ShellUserData {
        shell_data: shell_data.clone(),
        client_data: Mutex::new(make_shell_client_data()),
    });
    let mut user_impl = shell_data.user_impl.borrow_mut();
    (&mut *user_impl)(
        XdgRequest::NewClient {
            client: make_shell_client(&shell),
        },
        dispatch_data,
    );
    shell.deref().clone()
}

/*
 * xdg_shell
 */

pub(crate) struct ShellUserData {
    shell_data: ShellData,
    pub(crate) client_data: Mutex<ShellClientData>,
}

pub(crate) fn make_shell_client(resource: &zxdg_shell_v6::ZxdgShellV6) -> ShellClient {
    ShellClient {
        kind: super::ShellClientKind::ZxdgV6(resource.clone()),
    }
}

fn shell_implementation(
    shell: Main<zxdg_shell_v6::ZxdgShellV6>,
    request: zxdg_shell_v6::Request,
    dispatch_data: DispatchData<'_>,
) {
    let data = shell.as_ref().user_data().get::<ShellUserData>().unwrap();
    match request {
        zxdg_shell_v6::Request::Destroy => {
            // all is handled by destructor
        }
        zxdg_shell_v6::Request::CreatePositioner { id } => {
            implement_positioner(id);
        }
        zxdg_shell_v6::Request::GetXdgSurface { id, surface } => {
            id.quick_assign(xdg_surface_implementation);
            id.assign_destructor(Filter::new(|surface, _, _data| destroy_surface(surface)));
            id.as_ref().user_data().set(|| XdgSurfaceUserData {
                shell_data: data.shell_data.clone(),
                wl_surface: surface,
                shell: shell.deref().clone(),
                has_active_role: AtomicBool::new(false),
            });
        }
        zxdg_shell_v6::Request::Pong { serial } => {
            let serial = Serial::from(serial);
            let valid = {
                let mut guard = data.client_data.lock().unwrap();
                if guard.pending_ping == Some(serial) {
                    guard.pending_ping = None;
                    true
                } else {
                    false
                }
            };
            if valid {
                let mut user_impl = data.shell_data.user_impl.borrow_mut();
                (&mut *user_impl)(
                    XdgRequest::ClientPong {
                        client: make_shell_client(&shell),
                    },
                    dispatch_data,
                );
            }
        }
        _ => unreachable!(),
    }
}

/*
 * xdg_positioner
 */

fn implement_positioner(
    positioner: Main<zxdg_positioner_v6::ZxdgPositionerV6>,
) -> zxdg_positioner_v6::ZxdgPositionerV6 {
    positioner.quick_assign(|positioner, request, _data| {
        let mutex = positioner
            .as_ref()
            .user_data()
            .get::<RefCell<PositionerState>>()
            .unwrap();
        let mut state = mutex.borrow_mut();
        match request {
            zxdg_positioner_v6::Request::Destroy => {
                // handled by destructor
            }
            zxdg_positioner_v6::Request::SetSize { width, height } => {
                if width < 1 || height < 1 {
                    positioner.as_ref().post_error(
                        zxdg_positioner_v6::Error::InvalidInput as u32,
                        "Invalid size for positioner.".into(),
                    );
                } else {
                    state.rect_size = (width, height).into();
                }
            }
            zxdg_positioner_v6::Request::SetAnchorRect { x, y, width, height } => {
                if width < 1 || height < 1 {
                    positioner.as_ref().post_error(
                        zxdg_positioner_v6::Error::InvalidInput as u32,
                        "Invalid size for positioner's anchor rectangle.".into(),
                    );
                } else {
                    state.anchor_rect = Rectangle::from_loc_and_size((x, y), (width, height));
                }
            }
            zxdg_positioner_v6::Request::SetAnchor { anchor } => {
                if let Some(anchor) = zxdg_anchor_to_xdg(anchor) {
                    state.anchor_edges = anchor;
                } else {
                    positioner.as_ref().post_error(
                        zxdg_positioner_v6::Error::InvalidInput as u32,
                        "Invalid anchor for positioner.".into(),
                    );
                }
            }
            zxdg_positioner_v6::Request::SetGravity { gravity } => {
                if let Some(gravity) = zxdg_gravity_to_xdg(gravity) {
                    state.gravity = gravity;
                } else {
                    positioner.as_ref().post_error(
                        zxdg_positioner_v6::Error::InvalidInput as u32,
                        "Invalid gravity for positioner.".into(),
                    );
                }
            }
            zxdg_positioner_v6::Request::SetConstraintAdjustment {
                constraint_adjustment,
            } => {
                let constraint_adjustment =
                    zxdg_positioner_v6::ConstraintAdjustment::from_bits_truncate(constraint_adjustment);
                state.constraint_adjustment = zxdg_constraints_adg_to_xdg(constraint_adjustment);
            }
            zxdg_positioner_v6::Request::SetOffset { x, y } => {
                state.offset = (x, y).into();
            }
            _ => unreachable!(),
        }
    });
    positioner
        .as_ref()
        .user_data()
        .set(|| RefCell::new(PositionerState::default()));

    positioner.deref().clone()
}

/*
 * xdg_surface
 */

struct XdgSurfaceUserData {
    shell_data: ShellData,
    wl_surface: wl_surface::WlSurface,
    shell: zxdg_shell_v6::ZxdgShellV6,
    has_active_role: AtomicBool,
}

fn destroy_surface(surface: zxdg_surface_v6::ZxdgSurfaceV6) {
    let data = surface.as_ref().user_data().get::<XdgSurfaceUserData>().unwrap();
    if !data.wl_surface.as_ref().is_alive() {
        // the wl_surface is destroyed, this means the client is not
        // trying to change the role but it's a cleanup (possibly a
        // disconnecting client), ignore the protocol check.
        return;
    }

    if compositor::get_role(&data.wl_surface).is_none() {
        // No role assigned to the surface, we can exit early.
        return;
    }

    if data.has_active_role.load(Ordering::Acquire) {
        data.shell.as_ref().post_error(
            zxdg_shell_v6::Error::Role as u32,
            "xdg_surface was destroyed before its role object".into(),
        );
    }
}

fn xdg_surface_implementation(
    xdg_surface: Main<zxdg_surface_v6::ZxdgSurfaceV6>,
    request: zxdg_surface_v6::Request,
    dispatch_data: DispatchData<'_>,
) {
    let data = xdg_surface
        .as_ref()
        .user_data()
        .get::<XdgSurfaceUserData>()
        .unwrap();
    match request {
        zxdg_surface_v6::Request::Destroy => {
            // all is handled by our destructor
        }
        zxdg_surface_v6::Request::GetToplevel { id } => {
            // We now can assign a role to the surface
            let surface = &data.wl_surface;
            let shell = &data.shell;

            if compositor::give_role(surface, ZXDG_TOPLEVEL_ROLE).is_err() {
                shell.as_ref().post_error(
                    zxdg_shell_v6::Error::Role as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }

            data.has_active_role.store(true, Ordering::Release);

            compositor::with_states(surface, |states| {
                states
                    .data_map
                    .insert_if_missing_threadsafe(|| Mutex::new(XdgToplevelSurfaceRoleAttributes::default()))
            })
            .unwrap();

            compositor::add_commit_hook(surface, super::ToplevelSurface::commit_hook);

            id.quick_assign(toplevel_implementation);
            id.assign_destructor(Filter::new(|toplevel, _, _data| destroy_toplevel(toplevel)));
            id.as_ref().user_data().set(|| ShellSurfaceUserData {
                shell_data: data.shell_data.clone(),
                wl_surface: data.wl_surface.clone(),
                shell: data.shell.clone(),
                xdg_surface: xdg_surface.deref().clone(),
            });

            data.shell_data
                .shell_state
                .lock()
                .unwrap()
                .known_toplevels
                .push(make_toplevel_handle(&id));

            let handle = make_toplevel_handle(&id);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::NewToplevel { surface: handle }, dispatch_data);
        }
        zxdg_surface_v6::Request::GetPopup {
            id,
            parent,
            positioner,
        } => {
            let positioner_data = positioner
                .as_ref()
                .user_data()
                .get::<RefCell<PositionerState>>()
                .unwrap()
                .borrow()
                .clone();

            let parent_surface = {
                let parent_data = parent.as_ref().user_data().get::<XdgSurfaceUserData>().unwrap();
                parent_data.wl_surface.clone()
            };

            // We now can assign a role to the surface
            let surface = &data.wl_surface;
            let shell = &data.shell;

            let attributes = XdgPopupSurfaceRoleAttributes {
                parent: Some(parent_surface),
                server_pending: Some(PopupState {
                    // Set the positioner data as the popup geometry
                    geometry: positioner_data.get_geometry(),
                }),
                ..Default::default()
            };
            if compositor::give_role(surface, ZXDG_POPUP_ROLE).is_err() {
                shell.as_ref().post_error(
                    zxdg_shell_v6::Error::Role as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }

            data.has_active_role.store(true, Ordering::Release);

            compositor::with_states(surface, |states| {
                states
                    .data_map
                    .insert_if_missing_threadsafe(|| Mutex::new(XdgPopupSurfaceRoleAttributes::default()));
                *states
                    .data_map
                    .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                    .unwrap()
                    .lock()
                    .unwrap() = attributes;
            })
            .unwrap();

            compositor::add_commit_hook(surface, super::PopupSurface::commit_hook);

            id.quick_assign(popup_implementation);
            id.assign_destructor(Filter::new(|popup, _, _data| destroy_popup(popup)));
            id.as_ref().user_data().set(|| ShellSurfaceUserData {
                shell_data: data.shell_data.clone(),
                wl_surface: data.wl_surface.clone(),
                shell: data.shell.clone(),
                xdg_surface: xdg_surface.deref().clone(),
            });

            data.shell_data
                .shell_state
                .lock()
                .unwrap()
                .known_popups
                .push(make_popup_handle(&id));

            let handle = make_popup_handle(&id);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::NewPopup { surface: handle }, dispatch_data);
        }
        zxdg_surface_v6::Request::SetWindowGeometry { x, y, width, height } => {
            // Check the role of the surface, this can be either xdg_toplevel
            // or xdg_popup. If none of the role matches the xdg_surface has no role set
            // which is a protocol error.
            let surface = &data.wl_surface;

            let role = compositor::get_role(surface);

            if role.is_none() {
                xdg_surface.as_ref().post_error(
                    zxdg_surface_v6::Error::NotConstructed as u32,
                    "xdg_surface must have a role.".into(),
                );
                return;
            }

            if role != Some(ZXDG_TOPLEVEL_ROLE) && role != Some(ZXDG_POPUP_ROLE) {
                data.shell.as_ref().post_error(
                    zxdg_shell_v6::Error::Role as u32,
                    "xdg_surface must have a role of xdg_toplevel or xdg_popup.".into(),
                );
            }

            compositor::with_states(surface, |states| {
                states.cached_state.pending::<SurfaceCachedState>().geometry =
                    Some(Rectangle::from_loc_and_size((x, y), (width, height)));
            })
            .unwrap();
        }
        zxdg_surface_v6::Request::AckConfigure { serial } => {
            let serial = Serial::from(serial);
            let surface = &data.wl_surface;

            // Check the role of the surface, this can be either xdg_toplevel
            // or xdg_popup. If none of the role matches the xdg_surface has no role set
            // which is a protocol error.
            if compositor::get_role(surface).is_none() {
                data.shell.as_ref().post_error(
                    zxdg_surface_v6::Error::NotConstructed as u32,
                    "xdg_surface must have a role.".into(),
                );
                return;
            }

            // Find the correct configure state for the provided serial
            // discard all configure states that are older than the provided
            // serial.
            // If no matching serial can be found raise a protocol error
            //
            // Invoke the user impl with the found configuration
            // This has to include the serial and the role specific data.
            // - For xdg_popup there is no data.
            // - For xdg_toplevel send the state data including
            //   width, height, min/max size, maximized, fullscreen, resizing, activated
            //
            // This can be used to integrate custom protocol extensions
            //
            let found_configure = compositor::with_states(surface, |states| {
                if states.role == Some(ZXDG_TOPLEVEL_ROLE) {
                    Ok(states
                        .data_map
                        .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .ack_configure(serial))
                } else if states.role == Some(ZXDG_POPUP_ROLE) {
                    Ok(states
                        .data_map
                        .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .ack_configure(serial))
                } else {
                    Err(())
                }
            })
            .unwrap();
            let configure = match found_configure {
                Ok(Some(configure)) => configure,
                Ok(None) => {
                    data.shell.as_ref().post_error(
                        zxdg_shell_v6::Error::InvalidSurfaceState as u32,
                        format!("wrong configure serial: {}", <u32>::from(serial)),
                    );
                    return;
                }
                Err(_) => {
                    data.shell.as_ref().post_error(
                        zxdg_shell_v6::Error::Role as u32,
                        "xdg_surface must have a role of xdg_toplevel or xdg_popup.".into(),
                    );
                    return;
                }
            };

            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(
                XdgRequest::AckConfigure {
                    surface: surface.clone(),
                    configure,
                },
                dispatch_data,
            );
        }
        _ => unreachable!(),
    }
}

/*
 * xdg_toplevel
 */

pub struct ShellSurfaceUserData {
    pub(crate) shell_data: ShellData,
    pub(crate) wl_surface: wl_surface::WlSurface,
    pub(crate) shell: zxdg_shell_v6::ZxdgShellV6,
    pub(crate) xdg_surface: zxdg_surface_v6::ZxdgSurfaceV6,
}

// Utility functions allowing to factor out a lot of the upcoming logic
fn with_surface_toplevel_role_data<F, T>(toplevel: &zxdg_toplevel_v6::ZxdgToplevelV6, f: F) -> T
where
    F: FnOnce(&mut XdgToplevelSurfaceRoleAttributes) -> T,
{
    let data = toplevel
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();
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

fn with_toplevel_pending_state<F, T>(toplevel: &zxdg_toplevel_v6::ZxdgToplevelV6, f: F) -> T
where
    F: FnOnce(&mut SurfaceCachedState) -> T,
{
    let data = toplevel
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();
    compositor::with_states(&data.wl_surface, |states| {
        f(&mut *states.cached_state.pending::<SurfaceCachedState>())
    })
    .unwrap()
}

pub fn send_toplevel_configure(resource: &zxdg_toplevel_v6::ZxdgToplevelV6, configure: ToplevelConfigure) {
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();

    let (width, height) = configure.state.size.unwrap_or_default().into();
    // convert the Vec<State> (which is really a Vec<u32>) into Vec<u8>
    let states = {
        let mut states: Vec<xdg_toplevel::State> = configure.state.states.into();
        let ptr = states.as_mut_ptr();
        let len = states.len();
        let cap = states.capacity();
        ::std::mem::forget(states);
        unsafe { Vec::from_raw_parts(ptr as *mut u8, len * 4, cap * 4) }
    };
    let serial = configure.serial;

    // Send the toplevel configure
    resource.configure(width, height, states);

    // Send the base xdg_surface configure event to mark
    // The configure as finished
    data.xdg_surface.configure(serial.into());
}

fn make_toplevel_handle(resource: &zxdg_toplevel_v6::ZxdgToplevelV6) -> super::ToplevelSurface {
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();
    super::ToplevelSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: ToplevelKind::ZxdgV6(resource.clone()),
    }
}

fn toplevel_implementation(
    toplevel: Main<zxdg_toplevel_v6::ZxdgToplevelV6>,
    request: zxdg_toplevel_v6::Request,
    dispatch_data: DispatchData<'_>,
) {
    let data = toplevel
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();
    match request {
        zxdg_toplevel_v6::Request::Destroy => {
            // all it done by the destructor
        }
        zxdg_toplevel_v6::Request::SetParent { parent } => {
            with_surface_toplevel_role_data(&toplevel, |data| {
                data.parent = parent.map(|toplevel_surface_parent| {
                    let parent_data = toplevel_surface_parent
                        .as_ref()
                        .user_data()
                        .get::<ShellSurfaceUserData>()
                        .unwrap();
                    parent_data.wl_surface.clone()
                })
            });
        }
        zxdg_toplevel_v6::Request::SetTitle { title } => {
            // Title is not double buffered, we can set it directly
            with_surface_toplevel_role_data(&toplevel, |data| {
                data.title = Some(title);
            });
        }
        zxdg_toplevel_v6::Request::SetAppId { app_id } => {
            // AppId is not double buffered, we can set it directly
            with_surface_toplevel_role_data(&toplevel, |role| {
                role.app_id = Some(app_id);
            });
        }
        zxdg_toplevel_v6::Request::ShowWindowMenu { seat, serial, x, y } => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            let serial = Serial::from(serial);
            (&mut *user_impl)(
                XdgRequest::ShowWindowMenu {
                    surface: handle,
                    seat,
                    serial,
                    location: (x, y).into(),
                },
                dispatch_data,
            );
        }
        zxdg_toplevel_v6::Request::Move { seat, serial } => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            let serial = Serial::from(serial);
            (&mut *user_impl)(
                XdgRequest::Move {
                    surface: handle,
                    seat,
                    serial,
                },
                dispatch_data,
            );
        }
        zxdg_toplevel_v6::Request::Resize { seat, serial, edges } => {
            let edges =
                zxdg_toplevel_v6::ResizeEdge::from_raw(edges).unwrap_or(zxdg_toplevel_v6::ResizeEdge::None);
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            let serial = Serial::from(serial);
            (&mut *user_impl)(
                XdgRequest::Resize {
                    surface: handle,
                    seat,
                    serial,
                    edges: zxdg_edges_to_xdg(edges),
                },
                dispatch_data,
            );
        }
        zxdg_toplevel_v6::Request::SetMaxSize { width, height } => {
            with_toplevel_pending_state(&toplevel, |toplevel_data| {
                toplevel_data.max_size = (width, height).into();
            });
        }
        zxdg_toplevel_v6::Request::SetMinSize { width, height } => {
            with_toplevel_pending_state(&toplevel, |toplevel_data| {
                toplevel_data.min_size = (width, height).into();
            });
        }
        zxdg_toplevel_v6::Request::SetMaximized => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Maximize { surface: handle }, dispatch_data);
        }
        zxdg_toplevel_v6::Request::UnsetMaximized => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::UnMaximize { surface: handle }, dispatch_data);
        }
        zxdg_toplevel_v6::Request::SetFullscreen { output } => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(
                XdgRequest::Fullscreen {
                    surface: handle,
                    output,
                },
                dispatch_data,
            );
        }
        zxdg_toplevel_v6::Request::UnsetFullscreen => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::UnFullscreen { surface: handle }, dispatch_data);
        }
        zxdg_toplevel_v6::Request::SetMinimized => {
            // This has to be handled by the compositor, may not be
            // supported and just ignored
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Minimize { surface: handle }, dispatch_data);
        }
        _ => unreachable!(),
    }
}

fn destroy_toplevel(toplevel: zxdg_toplevel_v6::ZxdgToplevelV6) {
    let data = toplevel
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();
    if let Some(data) = data.xdg_surface.as_ref().user_data().get::<XdgSurfaceUserData>() {
        data.has_active_role.store(false, Ordering::Release);
    }
    // remove this surface from the known ones (as well as any leftover dead surface)
    data.shell_data
        .shell_state
        .lock()
        .unwrap()
        .known_toplevels
        .retain(|other| other.alive());
}

/*
 * xdg_popup
 */

pub(crate) fn send_popup_configure(resource: &zxdg_popup_v6::ZxdgPopupV6, configure: PopupConfigure) {
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();

    let serial = configure.serial;
    let geometry = configure.state.geometry;

    // Send the popup configure
    resource.configure(geometry.loc.x, geometry.loc.y, geometry.size.w, geometry.size.h);

    // Send the base xdg_surface configure event to mark
    // the configure as finished
    data.xdg_surface.configure(serial.into());
}

fn make_popup_handle(resource: &zxdg_popup_v6::ZxdgPopupV6) -> super::PopupSurface {
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();
    super::PopupSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: PopupKind::ZxdgV6(resource.clone()),
    }
}

fn popup_implementation(
    popup: Main<zxdg_popup_v6::ZxdgPopupV6>,
    request: zxdg_popup_v6::Request,
    dispatch_data: DispatchData<'_>,
) {
    let data = popup.as_ref().user_data().get::<ShellSurfaceUserData>().unwrap();
    match request {
        zxdg_popup_v6::Request::Destroy => {
            // all is handled by our destructor
        }
        zxdg_popup_v6::Request::Grab { seat, serial } => {
            let handle = make_popup_handle(&popup);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            let serial = Serial::from(serial);
            (&mut *user_impl)(
                XdgRequest::Grab {
                    surface: handle,
                    seat,
                    serial,
                },
                dispatch_data,
            );
        }
        _ => unreachable!(),
    }
}

fn destroy_popup(popup: zxdg_popup_v6::ZxdgPopupV6) {
    let data = popup.as_ref().user_data().get::<ShellSurfaceUserData>().unwrap();
    if let Some(data) = data.xdg_surface.as_ref().user_data().get::<XdgSurfaceUserData>() {
        data.has_active_role.store(false, Ordering::Release);
    }
    // remove this surface from the known ones (as well as any leftover dead surface)
    data.shell_data
        .shell_state
        .lock()
        .unwrap()
        .known_popups
        .retain(|other| other.alive());
}

fn zxdg_edges_to_xdg(e: zxdg_toplevel_v6::ResizeEdge) -> xdg_toplevel::ResizeEdge {
    match e {
        zxdg_toplevel_v6::ResizeEdge::None => xdg_toplevel::ResizeEdge::None,
        zxdg_toplevel_v6::ResizeEdge::Top => xdg_toplevel::ResizeEdge::Top,
        zxdg_toplevel_v6::ResizeEdge::Bottom => xdg_toplevel::ResizeEdge::Bottom,
        zxdg_toplevel_v6::ResizeEdge::Left => xdg_toplevel::ResizeEdge::Left,
        zxdg_toplevel_v6::ResizeEdge::Right => xdg_toplevel::ResizeEdge::Right,
        zxdg_toplevel_v6::ResizeEdge::TopLeft => xdg_toplevel::ResizeEdge::TopLeft,
        zxdg_toplevel_v6::ResizeEdge::TopRight => xdg_toplevel::ResizeEdge::TopRight,
        zxdg_toplevel_v6::ResizeEdge::BottomLeft => xdg_toplevel::ResizeEdge::BottomLeft,
        zxdg_toplevel_v6::ResizeEdge::BottomRight => xdg_toplevel::ResizeEdge::BottomRight,
        _ => unreachable!(),
    }
}

fn zxdg_constraints_adg_to_xdg(
    c: zxdg_positioner_v6::ConstraintAdjustment,
) -> xdg_positioner::ConstraintAdjustment {
    xdg_positioner::ConstraintAdjustment::from_bits_truncate(c.bits())
}

fn zxdg_gravity_to_xdg(c: zxdg_positioner_v6::Gravity) -> Option<xdg_positioner::Gravity> {
    match c.bits() {
        0b0000 => Some(xdg_positioner::Gravity::None),
        0b0001 => Some(xdg_positioner::Gravity::Top),
        0b0010 => Some(xdg_positioner::Gravity::Bottom),
        0b0100 => Some(xdg_positioner::Gravity::Left),
        0b0101 => Some(xdg_positioner::Gravity::TopLeft),
        0b0110 => Some(xdg_positioner::Gravity::BottomLeft),
        0b1000 => Some(xdg_positioner::Gravity::Right),
        0b1001 => Some(xdg_positioner::Gravity::TopRight),
        0b1010 => Some(xdg_positioner::Gravity::BottomRight),
        _ => None,
    }
}

fn zxdg_anchor_to_xdg(c: zxdg_positioner_v6::Anchor) -> Option<xdg_positioner::Anchor> {
    match c.bits() {
        0b0000 => Some(xdg_positioner::Anchor::None),
        0b0001 => Some(xdg_positioner::Anchor::Top),
        0b0010 => Some(xdg_positioner::Anchor::Bottom),
        0b0100 => Some(xdg_positioner::Anchor::Left),
        0b0101 => Some(xdg_positioner::Anchor::TopLeft),
        0b0110 => Some(xdg_positioner::Anchor::BottomLeft),
        0b1000 => Some(xdg_positioner::Anchor::Right),
        0b1001 => Some(xdg_positioner::Anchor::TopRight),
        0b1010 => Some(xdg_positioner::Anchor::BottomRight),
        _ => None,
    }
}
