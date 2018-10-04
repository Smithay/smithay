use std::{cell::RefCell, sync::Mutex};

use wayland::compositor::{roles::*, CompositorToken};
use wayland_protocols::xdg_shell::server::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_server::{protocol::wl_surface, DisplayToken, NewResource, Resource};

use utils::Rectangle;

use super::{
    make_shell_client_data, PopupConfigure, PopupKind, PopupState, PositionerState, ShellClient,
    ShellClientData, ShellData, ToplevelConfigure, ToplevelKind, ToplevelState, XdgRequest,
    XdgSurfacePendingState, XdgSurfaceRole,
};

pub(crate) fn implement_wm_base<U, R, SD>(
    shell: NewResource<xdg_wm_base::XdgWmBase>,
    shell_data: &ShellData<U, R, SD>,
) -> Resource<xdg_wm_base::XdgWmBase>
where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: Default + 'static,
{
    let shell = shell.implement_nonsend(
        wm_implementation::<U, R, SD>,
        None::<fn(_)>,
        ShellUserData {
            shell_data: shell_data.clone(),
            client_data: Mutex::new(make_shell_client_data::<SD>()),
        },
        &shell_data.display_token,
    );
    let mut user_impl = shell_data.user_impl.borrow_mut();
    (&mut *user_impl)(XdgRequest::NewClient {
        client: make_shell_client(&shell, shell_data.compositor_token),
    });
    shell
}

/*
 * xdg_shell
 */

pub(crate) struct ShellUserData<U, R, SD> {
    shell_data: ShellData<U, R, SD>,
    pub(crate) client_data: Mutex<ShellClientData<SD>>,
}

pub(crate) fn make_shell_client<U, R, SD>(
    resource: &Resource<xdg_wm_base::XdgWmBase>,
    token: CompositorToken<U, R>,
) -> ShellClient<U, R, SD> {
    ShellClient {
        kind: super::ShellClientKind::Xdg(resource.clone()),
        _token: token,
        _data: ::std::marker::PhantomData,
    }
}

fn wm_implementation<U, R, SD>(request: xdg_wm_base::Request, shell: Resource<xdg_wm_base::XdgWmBase>)
where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
{
    let data = shell.user_data::<ShellUserData<U, R, SD>>().unwrap();
    match request {
        xdg_wm_base::Request::Destroy => {
            // all is handled by destructor
        }
        xdg_wm_base::Request::CreatePositioner { id } => {
            implement_positioner(id, &data.shell_data.display_token);
        }
        xdg_wm_base::Request::GetXdgSurface { id, surface } => {
            let role_data = XdgSurfaceRole {
                pending_state: XdgSurfacePendingState::None,
                window_geometry: None,
                pending_configures: Vec::new(),
                configured: false,
            };
            if data
                .shell_data
                .compositor_token
                .give_role_with(&surface, role_data)
                .is_err()
            {
                shell.post_error(
                    xdg_wm_base::Error::Role as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }
            id.implement_nonsend(
                xdg_surface_implementation::<U, R, SD>,
                Some(destroy_surface::<U, R, SD>),
                XdgSurfaceUserData {
                    shell_data: data.shell_data.clone(),
                    wl_surface: surface,
                    wm_base: shell.clone(),
                },
                &data.shell_data.display_token,
            );
        }
        xdg_wm_base::Request::Pong { serial } => {
            let valid = {
                let mut guard = data.client_data.lock().unwrap();
                if guard.pending_ping == serial {
                    guard.pending_ping = 0;
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
    }
}

/*
 * xdg_positioner
 */

fn implement_positioner(
    positioner: NewResource<xdg_positioner::XdgPositioner>,
    token: &DisplayToken,
) -> Resource<xdg_positioner::XdgPositioner> {
    positioner.implement_nonsend(
        |request, positioner: Resource<_>| {
            let mutex = positioner.user_data::<RefCell<PositionerState>>().unwrap();
            let mut state = mutex.borrow_mut();
            match request {
                xdg_positioner::Request::Destroy => {
                    // handled by destructor
                }
                xdg_positioner::Request::SetSize { width, height } => {
                    if width < 1 || height < 1 {
                        positioner.post_error(
                            xdg_positioner::Error::InvalidInput as u32,
                            "Invalid size for positioner.".into(),
                        );
                    } else {
                        state.rect_size = (width, height);
                    }
                }
                xdg_positioner::Request::SetAnchorRect { x, y, width, height } => {
                    if width < 1 || height < 1 {
                        positioner.post_error(
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
            }
        },
        None::<fn(_)>,
        RefCell::new(PositionerState::new()),
        token,
    )
}

/*
 * xdg_surface
 */

struct XdgSurfaceUserData<U, R, SD> {
    shell_data: ShellData<U, R, SD>,
    wl_surface: Resource<wl_surface::WlSurface>,
    wm_base: Resource<xdg_wm_base::XdgWmBase>,
}

fn destroy_surface<U, R, SD>(surface: Resource<xdg_surface::XdgSurface>)
where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
{
    let data = surface.user_data::<XdgSurfaceUserData<U, R, SD>>().unwrap();
    if !data.wl_surface.is_alive() {
        // the wl_surface is destroyed, this means the client is not
        // trying to change the role but it's a cleanup (possibly a
        // disconnecting client), ignore the protocol check.
        return;
    }
    data.shell_data
        .compositor_token
        .with_role_data::<XdgSurfaceRole, _, _>(&data.wl_surface, |rdata| {
            if let XdgSurfacePendingState::None = rdata.pending_state {
                // all is good
            } else {
                data.wm_base.post_error(
                    xdg_wm_base::Error::Role as u32,
                    "xdg_surface was destroyed before its role object".into(),
                );
            }
        }).expect("xdg_surface exists but surface has not shell_surface role?!");
}

fn xdg_surface_implementation<U, R, SD>(
    request: xdg_surface::Request,
    xdg_surface: Resource<xdg_surface::XdgSurface>,
) where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
{
    let data = xdg_surface.user_data::<XdgSurfaceUserData<U, R, SD>>().unwrap();
    match request {
        xdg_surface::Request::Destroy => {
            // all is handled by our destructor
        }
        xdg_surface::Request::GetToplevel { id } => {
            data.shell_data
                .compositor_token
                .with_role_data::<XdgSurfaceRole, _, _>(&data.wl_surface, |data| {
                    data.pending_state = XdgSurfacePendingState::Toplevel(ToplevelState {
                        parent: None,
                        title: String::new(),
                        app_id: String::new(),
                        min_size: (0, 0),
                        max_size: (0, 0),
                    });
                }).expect("xdg_surface exists but surface has not shell_surface role?!");
            let toplevel = id.implement_nonsend(
                toplevel_implementation::<U, R, SD>,
                Some(destroy_toplevel::<U, R, SD>),
                ShellSurfaceUserData {
                    shell_data: data.shell_data.clone(),
                    wl_surface: data.wl_surface.clone(),
                    xdg_surface: xdg_surface.clone(),
                    wm_base: data.wm_base.clone(),
                },
                &data.shell_data.display_token,
            );

            data.shell_data
                .shell_state
                .lock()
                .unwrap()
                .known_toplevels
                .push(make_toplevel_handle(&toplevel));

            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::NewToplevel { surface: handle });
        }
        xdg_surface::Request::GetPopup {
            id,
            parent,
            positioner,
        } => {
            let positioner_data = positioner.user_data::<RefCell<PositionerState>>().unwrap();

            let parent_surface = parent.map(|parent| {
                let parent_data = parent.user_data::<XdgSurfaceUserData<U, R, SD>>().unwrap();
                parent_data.wl_surface.clone()
            });
            data.shell_data
                .compositor_token
                .with_role_data::<XdgSurfaceRole, _, _>(&data.wl_surface, |data| {
                    data.pending_state = XdgSurfacePendingState::Popup(PopupState {
                        parent: parent_surface,
                        positioner: positioner_data.borrow().clone(),
                    });
                }).expect("xdg_surface exists but surface has not shell_surface role?!");
            let popup = id.implement_nonsend(
                xg_popup_implementation::<U, R, SD>,
                Some(destroy_popup::<U, R, SD>),
                ShellSurfaceUserData {
                    shell_data: data.shell_data.clone(),
                    wl_surface: data.wl_surface.clone(),
                    xdg_surface: xdg_surface.clone(),
                    wm_base: data.wm_base.clone(),
                },
                &data.shell_data.display_token,
            );

            data.shell_data
                .shell_state
                .lock()
                .unwrap()
                .known_popups
                .push(make_popup_handle(&popup));

            let handle = make_popup_handle(&popup);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::NewPopup { surface: handle });
        }
        xdg_surface::Request::SetWindowGeometry { x, y, width, height } => {
            data.shell_data
                .compositor_token
                .with_role_data::<XdgSurfaceRole, _, _>(&data.wl_surface, |data| {
                    data.window_geometry = Some(Rectangle { x, y, width, height });
                }).expect("xdg_surface exists but surface has not shell_surface role?!");
        }
        xdg_surface::Request::AckConfigure { serial } => {
            data.shell_data
                .compositor_token
                .with_role_data::<XdgSurfaceRole, _, _>(&data.wl_surface, |role_data| {
                    let mut found = false;
                    role_data.pending_configures.retain(|&s| {
                        if s == serial {
                            found = true;
                        }
                        s > serial
                    });
                    if !found {
                        // client responded to a non-existing configure
                        data.wm_base.post_error(
                            xdg_wm_base::Error::InvalidSurfaceState as u32,
                            format!("Wrong configure serial: {}", serial),
                        );
                    }
                    role_data.configured = true;
                }).expect("xdg_surface exists but surface has not shell_surface role?!");
        }
    }
}

/*
 * xdg_toplevel
 */

pub(crate) struct ShellSurfaceUserData<U, R, SD> {
    pub(crate) shell_data: ShellData<U, R, SD>,
    pub(crate) wl_surface: Resource<wl_surface::WlSurface>,
    pub(crate) wm_base: Resource<xdg_wm_base::XdgWmBase>,
    pub(crate) xdg_surface: Resource<xdg_surface::XdgSurface>,
}

// Utility functions allowing to factor out a lot of the upcoming logic
fn with_surface_toplevel_data<U, R, SD, F>(
    shell_data: &ShellData<U, R, SD>,
    toplevel: &Resource<xdg_toplevel::XdgToplevel>,
    f: F,
) where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
    F: FnOnce(&mut ToplevelState),
{
    let toplevel_data = toplevel.user_data::<ShellSurfaceUserData<U, R, SD>>().unwrap();
    shell_data
        .compositor_token
        .with_role_data::<XdgSurfaceRole, _, _>(&toplevel_data.wl_surface, |data| match data.pending_state {
            XdgSurfacePendingState::Toplevel(ref mut toplevel_data) => f(toplevel_data),
            _ => unreachable!(),
        }).expect("xdg_toplevel exists but surface has not shell_surface role?!");
}

pub fn send_toplevel_configure<U, R, SD>(
    resource: &Resource<xdg_toplevel::XdgToplevel>,
    configure: ToplevelConfigure,
) where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
{
    let data = resource.user_data::<ShellSurfaceUserData<U, R, SD>>().unwrap();
    let (width, height) = configure.size.unwrap_or((0, 0));
    // convert the Vec<State> (which is really a Vec<u32>) into Vec<u8>
    let states = {
        let mut states = configure.states;
        let ptr = states.as_mut_ptr();
        let len = states.len();
        let cap = states.capacity();
        ::std::mem::forget(states);
        unsafe { Vec::from_raw_parts(ptr as *mut u8, len * 4, cap * 4) }
    };
    let serial = configure.serial;
    resource.send(xdg_toplevel::Event::Configure {
        width,
        height,
        states,
    });
    data.xdg_surface.send(xdg_surface::Event::Configure { serial });
    // Add the configure as pending
    data.shell_data
        .compositor_token
        .with_role_data::<XdgSurfaceRole, _, _>(&data.wl_surface, |data| data.pending_configures.push(serial))
        .expect("xdg_toplevel exists but surface has not shell_surface role?!");
}

fn make_toplevel_handle<U: 'static, R: 'static, SD: 'static>(
    resource: &Resource<xdg_toplevel::XdgToplevel>,
) -> super::ToplevelSurface<U, R, SD> {
    let data = resource.user_data::<ShellSurfaceUserData<U, R, SD>>().unwrap();
    super::ToplevelSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: ToplevelKind::Xdg(resource.clone()),
        token: data.shell_data.compositor_token,
        _shell_data: ::std::marker::PhantomData,
    }
}

fn toplevel_implementation<U, R, SD>(
    request: xdg_toplevel::Request,
    toplevel: Resource<xdg_toplevel::XdgToplevel>,
) where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
{
    let data = toplevel.user_data::<ShellSurfaceUserData<U, R, SD>>().unwrap();
    match request {
        xdg_toplevel::Request::Destroy => {
            // all it done by the destructor
        }
        xdg_toplevel::Request::SetParent { parent } => {
            with_surface_toplevel_data(&data.shell_data, &toplevel, |toplevel_data| {
                toplevel_data.parent = parent.map(|toplevel_surface_parent| {
                    toplevel_surface_parent
                        .user_data::<ShellSurfaceUserData<U, R, SD>>()
                        .unwrap()
                        .wl_surface
                        .clone()
                })
            });
        }
        xdg_toplevel::Request::SetTitle { title } => {
            with_surface_toplevel_data(&data.shell_data, &toplevel, |toplevel_data| {
                toplevel_data.title = title;
            });
        }
        xdg_toplevel::Request::SetAppId { app_id } => {
            with_surface_toplevel_data(&data.shell_data, &toplevel, |toplevel_data| {
                toplevel_data.app_id = app_id;
            });
        }
        xdg_toplevel::Request::ShowWindowMenu { seat, serial, x, y } => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::ShowWindowMenu {
                surface: handle,
                seat,
                serial,
                location: (x, y),
            });
        }
        xdg_toplevel::Request::Move { seat, serial } => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Move {
                surface: handle,
                seat,
                serial,
            });
        }
        xdg_toplevel::Request::Resize { seat, serial, edges } => {
            let edges = xdg_toplevel::ResizeEdge::from_raw(edges).unwrap_or(xdg_toplevel::ResizeEdge::None);
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Resize {
                surface: handle,
                seat,
                serial,
                edges,
            });
        }
        xdg_toplevel::Request::SetMaxSize { width, height } => {
            with_surface_toplevel_data(&data.shell_data, &toplevel, |toplevel_data| {
                toplevel_data.max_size = (width, height);
            });
        }
        xdg_toplevel::Request::SetMinSize { width, height } => {
            with_surface_toplevel_data(&data.shell_data, &toplevel, |toplevel_data| {
                toplevel_data.max_size = (width, height);
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
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Minimize { surface: handle });
        }
    }
}

fn destroy_toplevel<U, R, SD>(toplevel: Resource<xdg_toplevel::XdgToplevel>)
where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
{
    let data = toplevel.user_data::<ShellSurfaceUserData<U, R, SD>>().unwrap();
    if !data.wl_surface.is_alive() {
        // the wl_surface is destroyed, this means the client is not
        // trying to change the role but it's a cleanup (possibly a
        // disconnecting client), ignore the protocol check.
        return;
    } else {
        data.shell_data
            .compositor_token
            .with_role_data::<XdgSurfaceRole, _, _>(&data.wl_surface, |data| {
                data.pending_state = XdgSurfacePendingState::None;
                data.configured = false;
            }).expect("xdg_toplevel exists but surface has not shell_surface role?!");
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

pub(crate) fn send_popup_configure<U, R, SD>(
    resource: &Resource<xdg_popup::XdgPopup>,
    configure: PopupConfigure,
) where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
{
    let data = resource.user_data::<ShellSurfaceUserData<U, R, SD>>().unwrap();
    let (x, y) = configure.position;
    let (width, height) = configure.size;
    let serial = configure.serial;
    resource.send(xdg_popup::Event::Configure { x, y, width, height });
    data.xdg_surface.send(xdg_surface::Event::Configure { serial });
    // Add the configure as pending
    data.shell_data
        .compositor_token
        .with_role_data::<XdgSurfaceRole, _, _>(&data.wl_surface, |data| data.pending_configures.push(serial))
        .expect("xdg_toplevel exists but surface has not shell_surface role?!");
}

fn make_popup_handle<U: 'static, R: 'static, SD: 'static>(
    resource: &Resource<xdg_popup::XdgPopup>,
) -> super::PopupSurface<U, R, SD> {
    let data = resource.user_data::<ShellSurfaceUserData<U, R, SD>>().unwrap();
    super::PopupSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: PopupKind::Xdg(resource.clone()),
        token: data.shell_data.compositor_token,
        _shell_data: ::std::marker::PhantomData,
    }
}

fn xg_popup_implementation<U, R, SD>(request: xdg_popup::Request, popup: Resource<xdg_popup::XdgPopup>)
where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
{
    let data = popup.user_data::<ShellSurfaceUserData<U, R, SD>>().unwrap();
    match request {
        xdg_popup::Request::Destroy => {
            // all is handled by our destructor
        }
        xdg_popup::Request::Grab { seat, serial } => {
            let handle = make_popup_handle(&popup);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Grab {
                surface: handle,
                seat,
                serial,
            });
        }
    }
}

fn destroy_popup<U, R, SD>(popup: Resource<xdg_popup::XdgPopup>)
where
    U: 'static,
    R: Role<XdgSurfaceRole> + 'static,
    SD: 'static,
{
    let data = popup.user_data::<ShellSurfaceUserData<U, R, SD>>().unwrap();
    if !data.wl_surface.is_alive() {
        // the wl_surface is destroyed, this means the client is not
        // trying to change the role but it's a cleanup (possibly a
        // disconnecting client), ignore the protocol check.
        return;
    } else {
        data.shell_data
            .compositor_token
            .with_role_data::<XdgSurfaceRole, _, _>(&data.wl_surface, |data| {
                data.pending_state = XdgSurfacePendingState::None;
                data.configured = false;
            }).expect("xdg_popup exists but surface has not shell_surface role?!");
    }
    // remove this surface from the known ones (as well as any leftover dead surface)
    data.shell_data
        .shell_state
        .lock()
        .unwrap()
        .known_popups
        .retain(|other| other.alive());
}
