use std::sync::atomic::{AtomicBool, Ordering};
use std::{cell::RefCell, ops::Deref as _, sync::Mutex};

use crate::wayland::compositor;
use crate::wayland::shell::xdg::PopupState;
use crate::wayland::Serial;
use wayland_protocols::xdg_shell::server::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_server::DispatchData;
use wayland_server::{protocol::wl_surface, Filter, Main};

use crate::utils::Rectangle;

use super::{
    make_shell_client_data, PopupConfigure, PopupKind, PositionerState, ShellClient, ShellClientData,
    ShellData, SurfaceCachedState, ToplevelConfigure, ToplevelKind, XdgPopupSurfaceRoleAttributes,
    XdgRequest, XdgToplevelSurfaceRoleAttributes,
};

static XDG_TOPLEVEL_ROLE: &str = "xdg_toplevel";
static XDG_POPUP_ROLE: &str = "xdg_popup";

pub(crate) fn implement_wm_base(
    shell: Main<xdg_wm_base::XdgWmBase>,
    shell_data: &ShellData,
    dispatch_data: DispatchData<'_>,
) -> xdg_wm_base::XdgWmBase {
    shell.quick_assign(wm_implementation);
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

pub(crate) fn make_shell_client(resource: &xdg_wm_base::XdgWmBase) -> ShellClient {
    ShellClient {
        kind: super::ShellClientKind::Xdg(resource.clone()),
    }
}

fn wm_implementation(
    shell: Main<xdg_wm_base::XdgWmBase>,
    request: xdg_wm_base::Request,
    dispatch_data: DispatchData<'_>,
) {
    let data = shell.as_ref().user_data().get::<ShellUserData>().unwrap();
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
            id.quick_assign(|surface, req, dispatch_data| {
                xdg_surface_implementation(req, surface.deref().clone(), dispatch_data)
            });
            id.assign_destructor(Filter::new(|surface, _, _data| destroy_surface(surface)));
            id.as_ref().user_data().set(|| XdgSurfaceUserData {
                shell_data: data.shell_data.clone(),
                wl_surface: surface,
                wm_base: shell.deref().clone(),
                has_active_role: AtomicBool::new(false),
            });
        }
        xdg_wm_base::Request::Pong { serial } => {
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

struct XdgSurfaceUserData {
    shell_data: ShellData,
    wl_surface: wl_surface::WlSurface,
    wm_base: xdg_wm_base::XdgWmBase,
    has_active_role: AtomicBool,
}

fn destroy_surface(surface: xdg_surface::XdgSurface) {
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
        data.wm_base.as_ref().post_error(
            xdg_wm_base::Error::Role as u32,
            "xdg_surface was destroyed before its role object".into(),
        );
    }
}

fn xdg_surface_implementation(
    request: xdg_surface::Request,
    xdg_surface: xdg_surface::XdgSurface,
    dispatch_data: DispatchData<'_>,
) {
    let data = xdg_surface
        .as_ref()
        .user_data()
        .get::<XdgSurfaceUserData>()
        .unwrap();
    match request {
        xdg_surface::Request::Destroy => {
            // all is handled by our destructor
        }
        xdg_surface::Request::GetToplevel { id } => {
            // We now can assign a role to the surface
            let surface = &data.wl_surface;
            let shell = &data.wm_base;

            if compositor::give_role(surface, XDG_TOPLEVEL_ROLE).is_err() {
                shell.as_ref().post_error(
                    xdg_wm_base::Error::Role as u32,
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
            (&mut *user_impl)(XdgRequest::NewToplevel { surface: handle }, dispatch_data);
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
                let parent_data = parent.as_ref().user_data().get::<XdgSurfaceUserData>().unwrap();
                parent_data.wl_surface.clone()
            });

            // We now can assign a role to the surface
            let surface = &data.wl_surface;
            let shell = &data.wm_base;

            let attributes = XdgPopupSurfaceRoleAttributes {
                parent: parent_surface,
                server_pending: Some(PopupState {
                    // Set the positioner data as the popup geometry
                    geometry: positioner_data.get_geometry(),
                }),
                ..Default::default()
            };
            if compositor::give_role(surface, XDG_POPUP_ROLE).is_err() {
                shell.as_ref().post_error(
                    xdg_wm_base::Error::Role as u32,
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

            id.quick_assign(xdg_popup_implementation);
            id.assign_destructor(Filter::new(|popup, _, _data| destroy_popup(popup)));
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
            (&mut *user_impl)(XdgRequest::NewPopup { surface: handle }, dispatch_data);
        }
        xdg_surface::Request::SetWindowGeometry { x, y, width, height } => {
            // Check the role of the surface, this can be either xdg_toplevel
            // or xdg_popup. If none of the role matches the xdg_surface has no role set
            // which is a protocol error.
            let surface = &data.wl_surface;

            let role = compositor::get_role(surface);

            if role.is_none() {
                xdg_surface.as_ref().post_error(
                    xdg_surface::Error::NotConstructed as u32,
                    "xdg_surface must have a role.".into(),
                );
                return;
            }

            if role != Some(XDG_TOPLEVEL_ROLE) && role != Some(XDG_POPUP_ROLE) {
                data.wm_base.as_ref().post_error(
                    xdg_wm_base::Error::Role as u32,
                    "xdg_surface must have a role of xdg_toplevel or xdg_popup.".into(),
                );
            }

            compositor::with_states(surface, |states| {
                states.cached_state.pending::<SurfaceCachedState>().geometry =
                    Some(Rectangle { x, y, width, height });
            })
            .unwrap();
        }
        xdg_surface::Request::AckConfigure { serial } => {
            let serial = Serial::from(serial);
            let surface = &data.wl_surface;

            // Check the role of the surface, this can be either xdg_toplevel
            // or xdg_popup. If none of the role matches the xdg_surface has no role set
            // which is a protocol error.
            if compositor::get_role(surface).is_none() {
                xdg_surface.as_ref().post_error(
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
            let found_configure = compositor::with_states(surface, |states| {
                if states.role == Some(XDG_TOPLEVEL_ROLE) {
                    Ok(states
                        .data_map
                        .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .ack_configure(serial))
                } else if states.role == Some(XDG_POPUP_ROLE) {
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
                    data.wm_base.as_ref().post_error(
                        xdg_wm_base::Error::InvalidSurfaceState as u32,
                        format!("wrong configure serial: {}", <u32>::from(serial)),
                    );
                    return;
                }
                Err(()) => {
                    data.wm_base.as_ref().post_error(
                        xdg_wm_base::Error::Role as u32,
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

pub(crate) struct ShellSurfaceUserData {
    pub(crate) shell_data: ShellData,
    pub(crate) wl_surface: wl_surface::WlSurface,
    pub(crate) wm_base: xdg_wm_base::XdgWmBase,
    pub(crate) xdg_surface: xdg_surface::XdgSurface,
}

// Utility functions allowing to factor out a lot of the upcoming logic
fn with_surface_toplevel_role_data<F, T>(toplevel: &xdg_toplevel::XdgToplevel, f: F) -> T
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

fn with_toplevel_pending_state<F, T>(toplevel: &xdg_toplevel::XdgToplevel, f: F) -> T
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

pub fn send_toplevel_configure(resource: &xdg_toplevel::XdgToplevel, configure: ToplevelConfigure) {
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
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

fn make_toplevel_handle(resource: &xdg_toplevel::XdgToplevel) -> super::ToplevelSurface {
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();
    super::ToplevelSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: ToplevelKind::Xdg(resource.clone()),
    }
}

fn toplevel_implementation(
    toplevel: Main<xdg_toplevel::XdgToplevel>,
    request: xdg_toplevel::Request,
    dispatch_data: DispatchData<'_>,
) {
    let data = toplevel
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();
    match request {
        xdg_toplevel::Request::Destroy => {
            // all it done by the destructor
        }
        xdg_toplevel::Request::SetParent { parent } => {
            // Parent is not double buffered, we can set it directly
            with_surface_toplevel_role_data(&toplevel, |data| {
                data.parent = parent.map(|toplevel_surface_parent| {
                    toplevel_surface_parent
                        .as_ref()
                        .user_data()
                        .get::<ShellSurfaceUserData>()
                        .unwrap()
                        .wl_surface
                        .clone()
                })
            });
        }
        xdg_toplevel::Request::SetTitle { title } => {
            // Title is not double buffered, we can set it directly
            with_surface_toplevel_role_data(&toplevel, |data| {
                data.title = Some(title);
            });
        }
        xdg_toplevel::Request::SetAppId { app_id } => {
            // AppId is not double buffered, we can set it directly
            with_surface_toplevel_role_data(&toplevel, |role| {
                role.app_id = Some(app_id);
            });
        }
        xdg_toplevel::Request::ShowWindowMenu { seat, serial, x, y } => {
            // This has to be handled by the compositor
            let handle = make_toplevel_handle(&toplevel);
            let serial = Serial::from(serial);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(
                XdgRequest::ShowWindowMenu {
                    surface: handle,
                    seat,
                    serial,
                    location: (x, y),
                },
                dispatch_data,
            );
        }
        xdg_toplevel::Request::Move { seat, serial } => {
            // This has to be handled by the compositor
            let handle = make_toplevel_handle(&toplevel);
            let serial = Serial::from(serial);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(
                XdgRequest::Move {
                    surface: handle,
                    seat,
                    serial,
                },
                dispatch_data,
            );
        }
        xdg_toplevel::Request::Resize { seat, serial, edges } => {
            // This has to be handled by the compositor
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            let serial = Serial::from(serial);
            (&mut *user_impl)(
                XdgRequest::Resize {
                    surface: handle,
                    seat,
                    serial,
                    edges,
                },
                dispatch_data,
            );
        }
        xdg_toplevel::Request::SetMaxSize { width, height } => {
            with_toplevel_pending_state(&toplevel, |toplevel_data| {
                toplevel_data.max_size = (width, height);
            });
        }
        xdg_toplevel::Request::SetMinSize { width, height } => {
            with_toplevel_pending_state(&toplevel, |toplevel_data| {
                toplevel_data.min_size = (width, height);
            });
        }
        xdg_toplevel::Request::SetMaximized => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Maximize { surface: handle }, dispatch_data);
        }
        xdg_toplevel::Request::UnsetMaximized => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::UnMaximize { surface: handle }, dispatch_data);
        }
        xdg_toplevel::Request::SetFullscreen { output } => {
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
        xdg_toplevel::Request::UnsetFullscreen => {
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::UnFullscreen { surface: handle }, dispatch_data);
        }
        xdg_toplevel::Request::SetMinimized => {
            // This has to be handled by the compositor, may not be
            // supported and just ignored
            let handle = make_toplevel_handle(&toplevel);
            let mut user_impl = data.shell_data.user_impl.borrow_mut();
            (&mut *user_impl)(XdgRequest::Minimize { surface: handle }, dispatch_data);
        }
        _ => unreachable!(),
    }
}

fn destroy_toplevel(toplevel: xdg_toplevel::XdgToplevel) {
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

pub(crate) fn send_popup_configure(resource: &xdg_popup::XdgPopup, configure: PopupConfigure) {
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();

    let serial = configure.serial;
    let geometry = configure.state.geometry;

    // Send the popup configure
    resource.configure(geometry.x, geometry.y, geometry.width, geometry.height);

    // Send the base xdg_surface configure event to mark
    // the configure as finished
    data.xdg_surface.configure(serial.into());
}

fn make_popup_handle(resource: &xdg_popup::XdgPopup) -> super::PopupSurface {
    let data = resource
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();
    super::PopupSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: PopupKind::Xdg(resource.clone()),
    }
}

fn xdg_popup_implementation(
    popup: Main<xdg_popup::XdgPopup>,
    request: xdg_popup::Request,
    dispatch_data: DispatchData<'_>,
) {
    let data = popup.as_ref().user_data().get::<ShellSurfaceUserData>().unwrap();
    match request {
        xdg_popup::Request::Destroy => {
            // all is handled by our destructor
        }
        xdg_popup::Request::Grab { seat, serial } => {
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

fn destroy_popup(popup: xdg_popup::XdgPopup) {
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
