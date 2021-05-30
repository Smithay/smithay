use std::{cell::RefCell, ops::Deref as _, sync::Mutex};

use crate::wayland::compositor::{roles::*, CompositorToken};
use crate::wayland::shell::xdg::{ConfigureError, PopupState};
use crate::wayland::Serial;
use wayland_protocols::{
    unstable::xdg_shell::v6::server::{
        zxdg_popup_v6, zxdg_positioner_v6, zxdg_shell_v6, zxdg_surface_v6, zxdg_toplevel_v6,
    },
    xdg_shell::server::{xdg_positioner, xdg_toplevel},
};
use wayland_server::{protocol::wl_surface, Filter, Main};

use crate::utils::Rectangle;

use super::{
    make_shell_client_data, PopupConfigure, PopupKind, PositionerState, ShellClient, ShellClientData,
    ShellData, ToplevelClientPending, ToplevelConfigure, ToplevelKind, XdgPopupSurfaceRole,
    XdgPopupSurfaceRoleAttributes, XdgRequest, XdgToplevelSurfaceRole, XdgToplevelSurfaceRoleAttributes,
};

pub(crate) fn implement_shell<R>(
    shell: Main<zxdg_shell_v6::ZxdgShellV6>,
    shell_data: &ShellData<R>,
) -> zxdg_shell_v6::ZxdgShellV6
where
    R: Role<XdgToplevelSurfaceRole> + Role<XdgPopupSurfaceRole> + 'static,
{
    shell.quick_assign(|shell, req, _data| shell_implementation::<R>(req, shell.deref().clone()));
    shell.as_ref().user_data().set(|| ShellUserData {
        shell_data: shell_data.clone(),
        client_data: Mutex::new(make_shell_client_data()),
    });
    let mut user_impl = shell_data.user_impl.borrow_mut();
    (&mut *user_impl)(XdgRequest::NewClient {
        client: make_shell_client(&shell, shell_data.compositor_token),
    });
    shell.deref().clone()
}

/*
 * xdg_shell
 */

pub(crate) struct ShellUserData<R> {
    shell_data: ShellData<R>,
    pub(crate) client_data: Mutex<ShellClientData>,
}

pub(crate) fn make_shell_client<R>(
    resource: &zxdg_shell_v6::ZxdgShellV6,
    token: CompositorToken<R>,
) -> ShellClient<R> {
    ShellClient {
        kind: super::ShellClientKind::ZxdgV6(resource.clone()),
        _token: token,
    }
}

fn shell_implementation<R>(request: zxdg_shell_v6::Request, shell: zxdg_shell_v6::ZxdgShellV6)
where
    R: Role<XdgToplevelSurfaceRole> + Role<XdgPopupSurfaceRole> + 'static,
{
    let data = shell.as_ref().user_data().get::<ShellUserData<R>>().unwrap();
    match request {
        zxdg_shell_v6::Request::Destroy => {
            // all is handled by destructor
        }
        zxdg_shell_v6::Request::CreatePositioner { id } => {
            implement_positioner(id);
        }
        zxdg_shell_v6::Request::GetXdgSurface { id, surface } => {
            id.quick_assign(|surface, req, _data| {
                xdg_surface_implementation::<R>(req, surface.deref().clone())
            });
            id.assign_destructor(Filter::new(|surface, _, _data| destroy_surface::<R>(surface)));
            id.as_ref().user_data().set(|| XdgSurfaceUserData {
                shell_data: data.shell_data.clone(),
                wl_surface: surface,
                shell: shell.clone(),
            });
        }
        zxdg_shell_v6::Request::Pong { serial } => {
            let valid = {
                let mut guard = data.client_data.lock().unwrap();
                if guard.pending_ping == Serial::from(serial) {
                    guard.pending_ping = Serial::from(0);
                    true
                } else {
                    false
                }
            };
            if valid {
                let mut user_impl = data.shell_data.user_impl.borrow_mut();
                (&mut *user_impl)(XdgRequest::ClientPong {
                    client: make_shell_client(&shell, data.shell_data.compositor_token),
                });
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
                    state.rect_size = (width, height);
                }
            }
            zxdg_positioner_v6::Request::SetAnchorRect { x, y, width, height } => {
                if width < 1 || height < 1 {
                    positioner.as_ref().post_error(
                        zxdg_positioner_v6::Error::InvalidInput as u32,
                        "Invalid size for positioner's anchor rectangle.".into(),
                    );
                } else {
                    state.anchor_rect = Rectangle { x, y, width, height };
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
                state.offset = (x, y);
            }
            _ => unreachable!(),
        }
    });
    positioner
        .as_ref()
        .user_data()
        .set(|| RefCell::new(PositionerState::new()));

    positioner.deref().clone()
}

/*
 * xdg_surface
 */

struct XdgSurfaceUserData<R> {
    shell_data: ShellData<R>,
    wl_surface: wl_surface::WlSurface,
    shell: zxdg_shell_v6::ZxdgShellV6,
}

fn destroy_surface<R>(surface: zxdg_surface_v6::ZxdgSurfaceV6)
where
    R: Role<XdgToplevelSurfaceRole> + Role<XdgPopupSurfaceRole> + 'static,
{
    let data = surface
        .as_ref()
        .user_data()
        .get::<XdgSurfaceUserData<R>>()
        .unwrap();
    if !data.wl_surface.as_ref().is_alive() {
        // the wl_surface is destroyed, this means the client is not
        // trying to change the role but it's a cleanup (possibly a
        // disconnecting client), ignore the protocol check.
        return;
    }

    if !data.shell_data.compositor_token.has_a_role(&data.wl_surface) {
        // No role assigned to the surface, we can exit early.
        return;
    }

    let has_active_xdg_role = data
        .shell_data
        .compositor_token
        .with_role_data(&data.wl_surface, |role: &mut XdgToplevelSurfaceRole| {
            role.is_some()
        })
        .unwrap_or(false)
        || data
            .shell_data
            .compositor_token
            .with_role_data(&data.wl_surface, |role: &mut XdgPopupSurfaceRole| role.is_some())
            .unwrap_or(false);

    if has_active_xdg_role {
        data.shell.as_ref().post_error(
            zxdg_shell_v6::Error::Role as u32,
            "xdg_surface was destroyed before its role object".into(),
        );
    }
}

fn xdg_surface_implementation<R>(
    request: zxdg_surface_v6::Request,
    xdg_surface: zxdg_surface_v6::ZxdgSurfaceV6,
) where
    R: Role<XdgToplevelSurfaceRole> + Role<XdgPopupSurfaceRole> + 'static,
{
    let data = xdg_surface
        .as_ref()
        .user_data()
        .get::<XdgSurfaceUserData<R>>()
        .unwrap();
    match request {
        zxdg_surface_v6::Request::Destroy => {
            // all is handled by our destructor
        }
        zxdg_surface_v6::Request::GetToplevel { id } => {
            // We now can assign a role to the surface
            let surface = &data.wl_surface;
            let shell = &data.shell;

            let role_data = XdgToplevelSurfaceRole::Some(Default::default());

            if data
                .shell_data
                .compositor_token
                .give_role_with(&surface, role_data)
                .is_err()
            {
                shell.as_ref().post_error(
                    zxdg_shell_v6::Error::Role as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }

            id.quick_assign(|toplevel, req, _data| {
                toplevel_implementation::<R>(req, toplevel.deref().clone())
            });
            id.assign_destructor(Filter::new(|toplevel, _, _data| destroy_toplevel::<R>(toplevel)));
            id.as_ref().user_data().set(|| ShellSurfaceUserData {
                shell_data: data.shell_data.clone(),
                wl_surface: data.wl_surface.clone(),
                shell: data.shell.clone(),
                xdg_surface: xdg_surface.clone(),
            });

            data.shell_data
                .shell_state
                .lock()
                .unwrap()
                .known_toplevels
                .push(make_toplevel_handle(&id));

            let handle = make_toplevel_handle(&id);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::NewToplevel { surface: handle });
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
                let parent_data = parent
                    .as_ref()
                    .user_data()
                    .get::<XdgSurfaceUserData<R>>()
                    .unwrap();
                parent_data.wl_surface.clone()
            };

            // We now can assign a role to the surface
            let surface = &data.wl_surface;
            let shell = &data.shell;

            let role_data = XdgPopupSurfaceRole::Some(XdgPopupSurfaceRoleAttributes {
                parent: Some(parent_surface),
                server_pending: Some(PopupState {
                    // Set the positioner data as the popup geometry
                    geometry: positioner_data.get_geometry(),
                }),
                ..Default::default()
            });

            if data
                .shell_data
                .compositor_token
                .give_role_with(&surface, role_data)
                .is_err()
            {
                shell.as_ref().post_error(
                    zxdg_shell_v6::Error::Role as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }

            id.quick_assign(|popup, req, _data| popup_implementation::<R>(req, popup.deref().clone()));
            id.assign_destructor(Filter::new(|popup, _, _data| destroy_popup::<R>(popup)));
            id.as_ref().user_data().set(|| ShellSurfaceUserData {
                shell_data: data.shell_data.clone(),
                wl_surface: data.wl_surface.clone(),
                shell: data.shell.clone(),
                xdg_surface: xdg_surface.clone(),
            });

            data.shell_data
                .shell_state
                .lock()
                .unwrap()
                .known_popups
                .push(make_popup_handle(&id));

            let handle = make_popup_handle(&id);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::NewPopup { surface: handle });
        }
        zxdg_surface_v6::Request::SetWindowGeometry { x, y, width, height } => {
            // Check the role of the surface, this can be either xdg_toplevel
            // or xdg_popup. If none of the role matches the xdg_surface has no role set
            // which is a protocol error.
            let surface = &data.wl_surface;

            if !data.shell_data.compositor_token.has_a_role(surface) {
                data.shell.as_ref().post_error(
                    zxdg_surface_v6::Error::NotConstructed as u32,
                    "xdg_surface must have a role.".into(),
                );
                return;
            }

            // Set the next window geometry here, the geometry will be moved from
            // next to the current geometry on a commit. This has to be done currently
            // in anvil as the whole commit logic is implemented there until a proper
            // abstraction has been found to handle commits within roles. This also
            // ensures that a commit for a xdg_surface follows the rules for subsurfaces.
            let has_wrong_role = data
                .shell_data
                .compositor_token
                .with_xdg_role(surface, |role| {
                    role.set_window_geometry(Rectangle { x, y, width, height })
                })
                .is_err();

            if has_wrong_role {
                data.shell.as_ref().post_error(
                    zxdg_shell_v6::Error::Role as u32,
                    "xdg_surface must have a role of xdg_toplevel or xdg_popup.".into(),
                );
            }
        }
        zxdg_surface_v6::Request::AckConfigure { serial } => {
            let serial = Serial::from(serial);
            let surface = &data.wl_surface;

            // Check the role of the surface, this can be either xdg_toplevel
            // or xdg_popup. If none of the role matches the xdg_surface has no role set
            // which is a protocol error.
            if !data.shell_data.compositor_token.has_a_role(surface) {
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
            let configure = match data
                .shell_data
                .compositor_token
                .with_xdg_role(surface, |role| role.ack_configure(serial))
            {
                Ok(Ok(configure)) => configure,
                Ok(Err(ConfigureError::SerialNotFound(serial))) => {
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
            (&mut *user_impl)(XdgRequest::AckConfigure {
                surface: surface.clone(),
                configure,
            });
        }
        _ => unreachable!(),
    }
}

/*
 * xdg_toplevel
 */

pub struct ShellSurfaceUserData<R> {
    pub(crate) shell_data: ShellData<R>,
    pub(crate) wl_surface: wl_surface::WlSurface,
    pub(crate) shell: zxdg_shell_v6::ZxdgShellV6,
    pub(crate) xdg_surface: zxdg_surface_v6::ZxdgSurfaceV6,
}

// Utility functions allowing to factor out a lot of the upcoming logic
fn with_surface_toplevel_role_data<R, F, T>(
    shell_data: &ShellData<R>,
    toplevel: &zxdg_toplevel_v6::ZxdgToplevelV6,
    f: F,
) -> T
where
    R: Role<XdgToplevelSurfaceRole> + 'static,
    F: FnOnce(&mut XdgToplevelSurfaceRoleAttributes) -> T,
{
    let data = toplevel
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();
    shell_data
        .compositor_token
        .with_role_data::<XdgToplevelSurfaceRole, _, _>(&data.wl_surface, |role| {
            let attributes = role
                .as_mut()
                .expect("xdg_toplevel exists but role has been destroyed?!");
            f(attributes)
        })
        .expect("xdg_toplevel exists but surface has not shell_surface role?!")
}

fn with_surface_toplevel_client_pending<R, F, T>(
    shell_data: &ShellData<R>,
    toplevel: &zxdg_toplevel_v6::ZxdgToplevelV6,
    f: F,
) -> T
where
    R: Role<XdgToplevelSurfaceRole> + 'static,
    F: FnOnce(&mut ToplevelClientPending) -> T,
{
    with_surface_toplevel_role_data(shell_data, toplevel, |data| {
        if data.client_pending.is_none() {
            data.client_pending = Some(Default::default());
        }
        f(&mut data.client_pending.as_mut().unwrap())
    })
}

pub fn send_toplevel_configure<R>(resource: &zxdg_toplevel_v6::ZxdgToplevelV6, configure: ToplevelConfigure)
where
    R: Role<XdgToplevelSurfaceRole> + 'static,
{
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();

    let (width, height) = configure.state.size.unwrap_or((0, 0));
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

fn make_toplevel_handle<R: 'static>(
    resource: &zxdg_toplevel_v6::ZxdgToplevelV6,
) -> super::ToplevelSurface<R> {
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();
    super::ToplevelSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: ToplevelKind::ZxdgV6(resource.clone()),
        token: data.shell_data.compositor_token,
    }
}

fn toplevel_implementation<R>(request: zxdg_toplevel_v6::Request, toplevel: zxdg_toplevel_v6::ZxdgToplevelV6)
where
    R: Role<XdgToplevelSurfaceRole> + 'static,
{
    let data = toplevel
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();
    match request {
        zxdg_toplevel_v6::Request::Destroy => {
            // all it done by the destructor
        }
        zxdg_toplevel_v6::Request::SetParent { parent } => {
            with_surface_toplevel_role_data(&data.shell_data, &toplevel, |data| {
                data.parent = parent.map(|toplevel_surface_parent| {
                    let parent_data = toplevel_surface_parent
                        .as_ref()
                        .user_data()
                        .get::<ShellSurfaceUserData<R>>()
                        .unwrap();
                    parent_data.wl_surface.clone()
                })
            });
        }
        zxdg_toplevel_v6::Request::SetTitle { title } => {
            // Title is not double buffered, we can set it directly
            with_surface_toplevel_role_data(&data.shell_data, &toplevel, |data| {
                data.title = Some(title);
            });
        }
        zxdg_toplevel_v6::Request::SetAppId { app_id } => {
            // AppId is not double buffered, we can set it directly
            with_surface_toplevel_role_data(&data.shell_data, &toplevel, |role| {
                role.app_id = Some(app_id);
            });
        }
        zxdg_toplevel_v6::Request::ShowWindowMenu { seat, serial, x, y } => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            let serial = Serial::from(serial);
            (&mut *user_impl)(XdgRequest::ShowWindowMenu {
                surface: handle,
                seat,
                serial,
                location: (x, y),
            });
        }
        zxdg_toplevel_v6::Request::Move { seat, serial } => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            let serial = Serial::from(serial);
            (&mut *user_impl)(XdgRequest::Move {
                surface: handle,
                seat,
                serial,
            });
        }
        zxdg_toplevel_v6::Request::Resize { seat, serial, edges } => {
            let edges =
                zxdg_toplevel_v6::ResizeEdge::from_raw(edges).unwrap_or(zxdg_toplevel_v6::ResizeEdge::None);
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            let serial = Serial::from(serial);
            (&mut *user_impl)(XdgRequest::Resize {
                surface: handle,
                seat,
                serial,
                edges: zxdg_edges_to_xdg(edges),
            });
        }
        zxdg_toplevel_v6::Request::SetMaxSize { width, height } => {
            with_surface_toplevel_client_pending(&data.shell_data, &toplevel, |toplevel_data| {
                toplevel_data.max_size = Some((width, height));
            });
        }
        zxdg_toplevel_v6::Request::SetMinSize { width, height } => {
            with_surface_toplevel_client_pending(&data.shell_data, &toplevel, |toplevel_data| {
                toplevel_data.min_size = Some((width, height));
            });
        }
        zxdg_toplevel_v6::Request::SetMaximized => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Maximize { surface: handle });
        }
        zxdg_toplevel_v6::Request::UnsetMaximized => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::UnMaximize { surface: handle });
        }
        zxdg_toplevel_v6::Request::SetFullscreen { output } => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Fullscreen {
                surface: handle,
                output,
            });
        }
        zxdg_toplevel_v6::Request::UnsetFullscreen => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::UnFullscreen { surface: handle });
        }
        zxdg_toplevel_v6::Request::SetMinimized => {
            // This has to be handled by the compositor, may not be
            // supported and just ignored
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Minimize { surface: handle });
        }
        _ => unreachable!(),
    }
}

fn destroy_toplevel<R>(toplevel: zxdg_toplevel_v6::ZxdgToplevelV6)
where
    R: Role<XdgToplevelSurfaceRole> + 'static,
{
    let data = toplevel
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();
    if !data.wl_surface.as_ref().is_alive() {
        // the wl_surface is destroyed, this means the client is not
        // trying to change the role but it's a cleanup (possibly a
        // disconnecting client), ignore the protocol check.
    } else {
        data.shell_data
            .compositor_token
            .with_role_data(&data.wl_surface, |role_data| {
                *role_data = XdgToplevelSurfaceRole::None;
            })
            .expect("xdg_toplevel exists but surface has not shell_surface role?!");
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

pub(crate) fn send_popup_configure<R>(resource: &zxdg_popup_v6::ZxdgPopupV6, configure: PopupConfigure)
where
    R: Role<XdgPopupSurfaceRole> + 'static,
{
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();

    let serial = configure.serial;
    let geometry = configure.state.geometry;

    // Send the popup configure
    resource.configure(geometry.x, geometry.y, geometry.width, geometry.height);

    // Send the base xdg_surface configure event to mark
    // the configure as finished
    data.xdg_surface.configure(serial.into());
}

fn make_popup_handle<R: 'static>(resource: &zxdg_popup_v6::ZxdgPopupV6) -> super::PopupSurface<R> {
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();
    super::PopupSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: PopupKind::ZxdgV6(resource.clone()),
        token: data.shell_data.compositor_token,
    }
}

fn popup_implementation<R>(request: zxdg_popup_v6::Request, popup: zxdg_popup_v6::ZxdgPopupV6)
where
    R: Role<XdgPopupSurfaceRole> + 'static,
{
    let data = popup
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();
    match request {
        zxdg_popup_v6::Request::Destroy => {
            // all is handled by our destructor
        }
        zxdg_popup_v6::Request::Grab { seat, serial } => {
            let handle = make_popup_handle(&popup);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            let serial = Serial::from(serial);
            (&mut *user_impl)(XdgRequest::Grab {
                surface: handle,
                seat,
                serial,
            });
        }
        _ => unreachable!(),
    }
}

fn destroy_popup<R>(popup: zxdg_popup_v6::ZxdgPopupV6)
where
    R: Role<XdgPopupSurfaceRole> + 'static,
{
    let data = popup
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();
    if !data.wl_surface.as_ref().is_alive() {
        // the wl_surface is destroyed, this means the client is not
        // trying to change the role but it's a cleanup (possibly a
        // disconnecting client), ignore the protocol check.
    } else {
        data.shell_data
            .compositor_token
            .with_role_data(&data.wl_surface, |role_data| {
                *role_data = XdgPopupSurfaceRole::None;
            })
            .expect("xdg_popup exists but surface has not shell_surface role?!");
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
