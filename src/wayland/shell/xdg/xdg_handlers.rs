use std::{cell::RefCell, ops::Deref as _, sync::Mutex};

use crate::wayland::compositor::{roles::*, CompositorToken};
use crate::wayland::shell::xdg::{ConfigureError, PopupState};
use crate::wayland::Serial;
use wayland_protocols::xdg_shell::server::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_server::{protocol::wl_surface, Filter, Main};

use crate::utils::Rectangle;

use super::{
    make_shell_client_data, PopupConfigure, PopupKind, PositionerState, ShellClient, ShellClientData,
    ShellData, ToplevelClientPending, ToplevelConfigure, ToplevelKind, XdgPopupSurfaceRole,
    XdgPopupSurfaceRoleAttributes, XdgRequest, XdgToplevelSurfaceRole, XdgToplevelSurfaceRoleAttributes,
};

pub(crate) fn implement_wm_base<R>(
    shell: Main<xdg_wm_base::XdgWmBase>,
    shell_data: &ShellData<R>,
) -> xdg_wm_base::XdgWmBase
where
    R: Role<XdgToplevelSurfaceRole> + Role<XdgPopupSurfaceRole> + 'static,
{
    shell.quick_assign(|shell, req, _data| wm_implementation::<R>(req, shell.deref().clone()));
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
    resource: &xdg_wm_base::XdgWmBase,
    token: CompositorToken<R>,
) -> ShellClient<R> {
    ShellClient {
        kind: super::ShellClientKind::Xdg(resource.clone()),
        _token: token,
    }
}

fn wm_implementation<R>(request: xdg_wm_base::Request, shell: xdg_wm_base::XdgWmBase)
where
    R: Role<XdgToplevelSurfaceRole> + Role<XdgPopupSurfaceRole> + 'static,
{
    let data = shell.as_ref().user_data().get::<ShellUserData<R>>().unwrap();
    match request {
        xdg_wm_base::Request::Destroy => {
            // all is handled by destructor
        }
        xdg_wm_base::Request::CreatePositioner { id } => {
            implement_positioner(id);
        }
        xdg_wm_base::Request::GetXdgSurface { id, surface } => {
            // Do not assign a role to the surface here
            // xdg_surface is not role, only xdg_toplevel and
            // xdg_popup are defined as roles
            id.quick_assign(|surface, req, _data| {
                xdg_surface_implementation::<R>(req, surface.deref().clone())
            });
            id.assign_destructor(Filter::new(|surface, _, _data| destroy_surface::<R>(surface)));
            id.as_ref().user_data().set(|| XdgSurfaceUserData {
                shell_data: data.shell_data.clone(),
                wl_surface: surface,
                wm_base: shell.clone(),
            });
        }
        xdg_wm_base::Request::Pong { serial } => {
            let serial = Serial::from(serial);
            let valid = {
                let mut guard = data.client_data.lock().unwrap();
                if guard.pending_ping == serial {
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

fn implement_positioner(positioner: Main<xdg_positioner::XdgPositioner>) -> xdg_positioner::XdgPositioner {
    positioner.quick_assign(|positioner, request, _data| {
        let mutex = positioner
            .as_ref()
            .user_data()
            .get::<RefCell<PositionerState>>()
            .unwrap();
        let mut state = mutex.borrow_mut();
        match request {
            xdg_positioner::Request::Destroy => {
                // handled by destructor
            }
            xdg_positioner::Request::SetSize { width, height } => {
                if width < 1 || height < 1 {
                    positioner.as_ref().post_error(
                        xdg_positioner::Error::InvalidInput as u32,
                        "Invalid size for positioner.".into(),
                    );
                } else {
                    state.rect_size = (width, height);
                }
            }
            xdg_positioner::Request::SetAnchorRect { x, y, width, height } => {
                if width < 1 || height < 1 {
                    positioner.as_ref().post_error(
                        xdg_positioner::Error::InvalidInput as u32,
                        "Invalid size for positioner's anchor rectangle.".into(),
                    );
                } else {
                    state.anchor_rect = Rectangle { x, y, width, height };
                }
            }
            xdg_positioner::Request::SetAnchor { anchor } => {
                state.anchor_edges = anchor;
            }
            xdg_positioner::Request::SetGravity { gravity } => {
                state.gravity = gravity;
            }
            xdg_positioner::Request::SetConstraintAdjustment {
                constraint_adjustment,
            } => {
                let constraint_adjustment =
                    xdg_positioner::ConstraintAdjustment::from_bits_truncate(constraint_adjustment);
                state.constraint_adjustment = constraint_adjustment;
            }
            xdg_positioner::Request::SetOffset { x, y } => {
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
    wm_base: xdg_wm_base::XdgWmBase,
}

fn destroy_surface<R>(surface: xdg_surface::XdgSurface)
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
        data.wm_base.as_ref().post_error(
            xdg_wm_base::Error::Role as u32,
            "xdg_surface was destroyed before its role object".into(),
        );
    }
}

fn xdg_surface_implementation<R>(request: xdg_surface::Request, xdg_surface: xdg_surface::XdgSurface)
where
    R: Role<XdgToplevelSurfaceRole> + Role<XdgPopupSurfaceRole> + 'static,
{
    let data = xdg_surface
        .as_ref()
        .user_data()
        .get::<XdgSurfaceUserData<R>>()
        .unwrap();
    match request {
        xdg_surface::Request::Destroy => {
            // all is handled by our destructor
        }
        xdg_surface::Request::GetToplevel { id } => {
            // We now can assign a role to the surface
            let surface = &data.wl_surface;
            let shell = &data.wm_base;

            let role_data = XdgToplevelSurfaceRole::Some(Default::default());

            if data
                .shell_data
                .compositor_token
                .give_role_with(&surface, role_data)
                .is_err()
            {
                shell.as_ref().post_error(
                    xdg_wm_base::Error::Role as u32,
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
                xdg_surface: xdg_surface.clone(),
                wm_base: data.wm_base.clone(),
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
        xdg_surface::Request::GetPopup {
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

            let parent_surface = parent.map(|parent| {
                let parent_data = parent
                    .as_ref()
                    .user_data()
                    .get::<XdgSurfaceUserData<R>>()
                    .unwrap();
                parent_data.wl_surface.clone()
            });

            // We now can assign a role to the surface
            let surface = &data.wl_surface;
            let shell = &data.wm_base;

            let role_data = XdgPopupSurfaceRole::Some(XdgPopupSurfaceRoleAttributes {
                parent: parent_surface,
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
                    xdg_wm_base::Error::Role as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }

            id.quick_assign(|popup, req, _data| xdg_popup_implementation::<R>(req, popup.deref().clone()));
            id.assign_destructor(Filter::new(|popup, _, _data| destroy_popup::<R>(popup)));
            id.as_ref().user_data().set(|| ShellSurfaceUserData {
                shell_data: data.shell_data.clone(),
                wl_surface: data.wl_surface.clone(),
                xdg_surface: xdg_surface.clone(),
                wm_base: data.wm_base.clone(),
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
        xdg_surface::Request::SetWindowGeometry { x, y, width, height } => {
            // Check the role of the surface, this can be either xdg_toplevel
            // or xdg_popup. If none of the role matches the xdg_surface has no role set
            // which is a protocol error.
            let surface = &data.wl_surface;

            if !data.shell_data.compositor_token.has_a_role(surface) {
                data.wm_base.as_ref().post_error(
                    xdg_surface::Error::NotConstructed as u32,
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
                data.wm_base.as_ref().post_error(
                    xdg_wm_base::Error::Role as u32,
                    "xdg_surface must have a role of xdg_toplevel or xdg_popup.".into(),
                );
            }
        }
        xdg_surface::Request::AckConfigure { serial } => {
            let serial = Serial::from(serial);
            let surface = &data.wl_surface;

            // Check the role of the surface, this can be either xdg_toplevel
            // or xdg_popup. If none of the role matches the xdg_surface has no role set
            // which is a protocol error.
            if !data.shell_data.compositor_token.has_a_role(surface) {
                data.wm_base.as_ref().post_error(
                    xdg_surface::Error::NotConstructed as u32,
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
                    data.wm_base.as_ref().post_error(
                        xdg_wm_base::Error::InvalidSurfaceState as u32,
                        format!("wrong configure serial: {}", <u32>::from(serial)),
                    );
                    return;
                }
                Err(_) => {
                    data.wm_base.as_ref().post_error(
                        xdg_wm_base::Error::Role as u32,
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

pub(crate) struct ShellSurfaceUserData<R> {
    pub(crate) shell_data: ShellData<R>,
    pub(crate) wl_surface: wl_surface::WlSurface,
    pub(crate) wm_base: xdg_wm_base::XdgWmBase,
    pub(crate) xdg_surface: xdg_surface::XdgSurface,
}

// Utility functions allowing to factor out a lot of the upcoming logic
fn with_surface_toplevel_role_data<R, F, T>(
    shell_data: &ShellData<R>,
    toplevel: &xdg_toplevel::XdgToplevel,
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
    toplevel: &xdg_toplevel::XdgToplevel,
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

pub fn send_toplevel_configure<R>(resource: &xdg_toplevel::XdgToplevel, configure: ToplevelConfigure)
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

fn make_toplevel_handle<R: 'static>(resource: &xdg_toplevel::XdgToplevel) -> super::ToplevelSurface<R> {
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();
    super::ToplevelSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: ToplevelKind::Xdg(resource.clone()),
        token: data.shell_data.compositor_token,
    }
}

fn toplevel_implementation<R>(request: xdg_toplevel::Request, toplevel: xdg_toplevel::XdgToplevel)
where
    R: Role<XdgToplevelSurfaceRole> + 'static,
{
    let data = toplevel
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();
    match request {
        xdg_toplevel::Request::Destroy => {
            // all it done by the destructor
        }
        xdg_toplevel::Request::SetParent { parent } => {
            // Parent is not double buffered, we can set it directly
            with_surface_toplevel_role_data(&data.shell_data, &toplevel, |data| {
                data.parent = parent.map(|toplevel_surface_parent| {
                    toplevel_surface_parent
                        .as_ref()
                        .user_data()
                        .get::<ShellSurfaceUserData<R>>()
                        .unwrap()
                        .wl_surface
                        .clone()
                })
            });
        }
        xdg_toplevel::Request::SetTitle { title } => {
            // Title is not double buffered, we can set it directly
            with_surface_toplevel_role_data(&data.shell_data, &toplevel, |data| {
                data.title = Some(title);
            });
        }
        xdg_toplevel::Request::SetAppId { app_id } => {
            // AppId is not double buffered, we can set it directly
            with_surface_toplevel_role_data(&data.shell_data, &toplevel, |role| {
                role.app_id = Some(app_id);
            });
        }
        xdg_toplevel::Request::ShowWindowMenu { seat, serial, x, y } => {
            // This has to be handled by the compositor
            let handle = make_toplevel_handle(&toplevel);
            let serial = Serial::from(serial);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::ShowWindowMenu {
                surface: handle,
                seat,
                serial,
                location: (x, y),
            });
        }
        xdg_toplevel::Request::Move { seat, serial } => {
            // This has to be handled by the compositor
            let handle = make_toplevel_handle(&toplevel);
            let serial = Serial::from(serial);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Move {
                surface: handle,
                seat,
                serial,
            });
        }
        xdg_toplevel::Request::Resize { seat, serial, edges } => {
            // This has to be handled by the compositor
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            let serial = Serial::from(serial);
            (&mut *user_impl)(XdgRequest::Resize {
                surface: handle,
                seat,
                serial,
                edges,
            });
        }
        xdg_toplevel::Request::SetMaxSize { width, height } => {
            with_surface_toplevel_client_pending(&data.shell_data, &toplevel, |toplevel_data| {
                toplevel_data.max_size = Some((width, height));
            });
        }
        xdg_toplevel::Request::SetMinSize { width, height } => {
            with_surface_toplevel_client_pending(&data.shell_data, &toplevel, |toplevel_data| {
                toplevel_data.min_size = Some((width, height));
            });
        }
        xdg_toplevel::Request::SetMaximized => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Maximize { surface: handle });
        }
        xdg_toplevel::Request::UnsetMaximized => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::UnMaximize { surface: handle });
        }
        xdg_toplevel::Request::SetFullscreen { output } => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Fullscreen {
                surface: handle,
                output,
            });
        }
        xdg_toplevel::Request::UnsetFullscreen => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::UnFullscreen { surface: handle });
        }
        xdg_toplevel::Request::SetMinimized => {
            // This has to be handled by the compositor, may not be
            // supported and just ignored
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Minimize { surface: handle });
        }
        _ => unreachable!(),
    }
}

fn destroy_toplevel<R>(toplevel: xdg_toplevel::XdgToplevel)
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

pub(crate) fn send_popup_configure<R>(resource: &xdg_popup::XdgPopup, configure: PopupConfigure)
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

fn make_popup_handle<R: 'static>(resource: &xdg_popup::XdgPopup) -> super::PopupSurface<R> {
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();
    super::PopupSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: PopupKind::Xdg(resource.clone()),
        token: data.shell_data.compositor_token,
    }
}

fn xdg_popup_implementation<R>(request: xdg_popup::Request, popup: xdg_popup::XdgPopup)
where
    R: Role<XdgPopupSurfaceRole> + 'static,
{
    let data = popup
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();
    match request {
        xdg_popup::Request::Destroy => {
            // all is handled by our destructor
        }
        xdg_popup::Request::Grab { seat, serial } => {
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

fn destroy_popup<R>(popup: xdg_popup::XdgPopup)
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
