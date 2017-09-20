use super::{make_shell_client_data, PopupConfigure, PopupState, PositionerState, ShellClient,
            ShellClientData, ShellSurfaceIData, ShellSurfacePendingState, ShellSurfaceRole,
            ToplevelConfigure, ToplevelState};
use compositor::{CompositorToken, Rectangle};
use compositor::roles::*;
use std::sync::Mutex;
use wayland_protocols::unstable::xdg_shell::server::{zxdg_popup_v6, zxdg_positioner_v6, zxdg_shell_v6,
                                                     zxdg_surface_v6, zxdg_toplevel_v6};
use wayland_server::{Client, EventLoopHandle, Resource};
use wayland_server::protocol::{wl_output, wl_surface};

pub(crate) fn xdg_shell_bind<U, R, CID, SID, SD>(evlh: &mut EventLoopHandle,
                                                 idata: &mut ShellSurfaceIData<U, R, CID, SID, SD>,
                                                 _: &Client, shell: zxdg_shell_v6::ZxdgShellV6)
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: Default + 'static,
{
    shell.set_user_data(
        Box::into_raw(Box::new(Mutex::new(make_shell_client_data::<SD>()))) as *mut _,
    );
    evlh.register(
        &shell,
        shell_implementation(),
        idata.clone(),
        Some(destroy_shell::<SD>),
    );
    let mut user_idata = idata.idata.borrow_mut();
    (idata.implementation.new_client)(evlh, &mut *user_idata, make_shell_client(&shell));
}

/*
 * xdg_shell
 */

pub(crate) type ShellUserData<SD> = Mutex<ShellClientData<SD>>;

fn destroy_shell<SD>(shell: &zxdg_shell_v6::ZxdgShellV6) {
    let ptr = shell.get_user_data();
    shell.set_user_data(::std::ptr::null_mut());
    let data = unsafe { Box::from_raw(ptr as *mut ShellUserData<SD>) };
    // explicit call to drop to not forget what we're doing here
    ::std::mem::drop(data);
}

pub(crate) fn make_shell_client<SD>(resource: &zxdg_shell_v6::ZxdgShellV6) -> ShellClient<SD> {
    ShellClient {
        kind: super::ShellClientKind::Xdg(unsafe { resource.clone_unchecked() }),
        _data: ::std::marker::PhantomData,
    }
}

fn shell_implementation<U, R, CID, SID, SD>(
    )
    -> zxdg_shell_v6::Implementation<ShellSurfaceIData<U, R, CID, SID, SD>>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: 'static,
{
    zxdg_shell_v6::Implementation {
        destroy: |_, _, _, _| {},
        create_positioner: |evlh, _, _, _, positioner| {
            let data = PositionerState {
                rect_size: (0, 0),
                anchor_rect: Rectangle {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                anchor_edges: zxdg_positioner_v6::Anchor::empty(),
                gravity: zxdg_positioner_v6::Gravity::empty(),
                constraint_adjustment: zxdg_positioner_v6::ConstraintAdjustment::empty(),
                offset: (0, 0),
            };
            positioner.set_user_data(Box::into_raw(Box::new(data)) as *mut _);
            evlh.register(
                &positioner,
                positioner_implementation(),
                (),
                Some(destroy_positioner),
            );
        },
        get_xdg_surface: |evlh, idata, _, shell, xdg_surface, wl_surface| {
            let role_data = ShellSurfaceRole {
                pending_state: ShellSurfacePendingState::None,
                window_geometry: None,
                pending_configures: Vec::new(),
                configured: false,
            };
            if let Err(_) = idata.compositor_token.give_role_with(wl_surface, role_data) {
                shell.post_error(
                    zxdg_shell_v6::Error::Role as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }
            xdg_surface.set_user_data(
                Box::into_raw(Box::new((unsafe { wl_surface.clone_unchecked() }, unsafe {
                    shell.clone_unchecked()
                }))) as *mut _,
            );
            evlh.register(
                &xdg_surface,
                surface_implementation(),
                idata.clone(),
                Some(destroy_surface),
            );
        },
        pong: |evlh, idata, _, shell, serial| {
            let valid = {
                let mutex = unsafe { &*(shell.get_user_data() as *mut ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                if guard.pending_ping == serial {
                    guard.pending_ping = 0;
                    true
                } else {
                    false
                }
            };
            if valid {
                let mut user_idata = idata.idata.borrow_mut();
                (idata.implementation.client_pong)(evlh, &mut *user_idata, make_shell_client(shell));
            }
        },
    }
}

/*
 * xdg_positioner
 */

fn destroy_positioner(positioner: &zxdg_positioner_v6::ZxdgPositionerV6) {
    let ptr = positioner.get_user_data();
    positioner.set_user_data(::std::ptr::null_mut());
    // drop the PositionerState
    let surface = unsafe { Box::from_raw(ptr as *mut PositionerState) };
    // explicit call to drop to not forget what we're doing here
    ::std::mem::drop(surface);
}

fn positioner_implementation() -> zxdg_positioner_v6::Implementation<()> {
    zxdg_positioner_v6::Implementation {
        destroy: |_, _, _, _| {},
        set_size: |_, _, _, positioner, width, height| if width < 1 || height < 1 {
            positioner.post_error(
                zxdg_positioner_v6::Error::InvalidInput as u32,
                "Invalid size for positioner.".into(),
            );
        } else {
            let ptr = positioner.get_user_data();
            let state = unsafe { &mut *(ptr as *mut PositionerState) };
            state.rect_size = (width, height);
        },
        set_anchor_rect: |_, _, _, positioner, x, y, width, height| if width < 1 || height < 1 {
            positioner.post_error(
                zxdg_positioner_v6::Error::InvalidInput as u32,
                "Invalid size for positioner's anchor rectangle.".into(),
            );
        } else {
            let ptr = positioner.get_user_data();
            let state = unsafe { &mut *(ptr as *mut PositionerState) };
            state.anchor_rect = Rectangle {
                x,
                y,
                width,
                height,
            };
        },
        set_anchor: |_, _, _, positioner, anchor| {
            use self::zxdg_positioner_v6::{AnchorBottom, AnchorLeft, AnchorRight, AnchorTop};
            if anchor.contains(AnchorLeft | AnchorRight) || anchor.contains(AnchorTop | AnchorBottom) {
                positioner.post_error(
                    zxdg_positioner_v6::Error::InvalidInput as u32,
                    "Invalid anchor for positioner.".into(),
                );
            } else {
                let ptr = positioner.get_user_data();
                let state = unsafe { &mut *(ptr as *mut PositionerState) };
                state.anchor_edges = anchor;
            }
        },
        set_gravity: |_, _, _, positioner, gravity| {
            use self::zxdg_positioner_v6::{GravityBottom, GravityLeft, GravityRight, GravityTop};
            if gravity.contains(GravityLeft | GravityRight) || gravity.contains(GravityTop | GravityBottom) {
                positioner.post_error(
                    zxdg_positioner_v6::Error::InvalidInput as u32,
                    "Invalid gravity for positioner.".into(),
                );
            } else {
                let ptr = positioner.get_user_data();
                let state = unsafe { &mut *(ptr as *mut PositionerState) };
                state.gravity = gravity;
            }
        },
        set_constraint_adjustment: |_, _, _, positioner, constraint_adjustment| {
            let constraint_adjustment =
                zxdg_positioner_v6::ConstraintAdjustment::from_bits_truncate(constraint_adjustment);
            let ptr = positioner.get_user_data();
            let state = unsafe { &mut *(ptr as *mut PositionerState) };
            state.constraint_adjustment = constraint_adjustment;
        },
        set_offset: |_, _, _, positioner, x, y| {
            let ptr = positioner.get_user_data();
            let state = unsafe { &mut *(ptr as *mut PositionerState) };
            state.offset = (x, y);
        },
    }
}

/*
 * xdg_surface
 */

fn destroy_surface(surface: &zxdg_surface_v6::ZxdgSurfaceV6) {
    let ptr = surface.get_user_data();
    surface.set_user_data(::std::ptr::null_mut());
    // drop the state
    let data = unsafe {
        Box::from_raw(
            ptr as *mut (zxdg_surface_v6::ZxdgSurfaceV6, zxdg_shell_v6::ZxdgShellV6),
        )
    };
    // explicit call to drop to not forget what we're doing here
    ::std::mem::drop(data);
}

fn surface_implementation<U, R, CID, SID, SD>(
    )
    -> zxdg_surface_v6::Implementation<ShellSurfaceIData<U, R, CID, SID, SD>>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: 'static,
{
    zxdg_surface_v6::Implementation {
        destroy: |_, idata, _, xdg_surface| {
            let ptr = xdg_surface.get_user_data();
            let &(ref surface, ref shell) =
                unsafe { &*(ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };
            idata
                .compositor_token
                .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                    if let ShellSurfacePendingState::None = data.pending_state {
                        // all is good
                    } else {
                        shell.post_error(
                            zxdg_shell_v6::Error::Role as u32,
                            "xdg_surface was destroyed before its role object".into(),
                        );
                    }
                })
                .expect(
                    "xdg_surface exists but surface has not shell_surface role?!",
                );
        },
        get_toplevel: |evlh, idata, _, xdg_surface, toplevel| {
            let ptr = xdg_surface.get_user_data();
            let &(ref surface, ref shell) =
                unsafe { &*(ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };
            idata
                .compositor_token
                .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                    data.pending_state = ShellSurfacePendingState::Toplevel(ToplevelState {
                        parent: None,
                        title: String::new(),
                        app_id: String::new(),
                        min_size: (0, 0),
                        max_size: (0, 0),
                    });
                })
                .expect(
                    "xdg_surface exists but surface has not shell_surface role?!",
                );

            toplevel.set_user_data(Box::into_raw(Box::new(unsafe {
                (
                    surface.clone_unchecked(),
                    shell.clone_unchecked(),
                    xdg_surface.clone_unchecked(),
                )
            })) as *mut _);
            evlh.register(
                &toplevel,
                toplevel_implementation(),
                idata.clone(),
                Some(destroy_toplevel),
            );

            // register to self
            evlh.state()
                .get_mut(&idata.state_token)
                .known_toplevels
                .push(make_toplevel_handle(idata.compositor_token, &toplevel));

            // intial configure event
            let handle = make_toplevel_handle(idata.compositor_token, &toplevel);
            let mut user_idata = idata.idata.borrow_mut();
            let configure = (idata.implementation.new_toplevel)(evlh, &mut *user_idata, handle);
            send_toplevel_configure(idata.compositor_token, &toplevel, configure);
        },
        get_popup: |evlh, idata, _, xdg_surface, popup, parent, positioner| {
            let ptr = xdg_surface.get_user_data();
            let &(ref surface, ref shell) =
                unsafe { &*(ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };

            let positioner_data = unsafe { &*(positioner.get_user_data() as *const PositionerState) };

            let parent_ptr = parent.get_user_data();
            let &(ref parent_surface, _) =
                unsafe { &*(parent_ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };

            idata
                .compositor_token
                .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                    data.pending_state = ShellSurfacePendingState::Popup(PopupState {
                        parent: unsafe { parent_surface.clone_unchecked() },
                        positioner: positioner_data.clone(),
                    });
                })
                .expect(
                    "xdg_surface exists but surface has not shell_surface role?!",
                );

            popup.set_user_data(Box::into_raw(Box::new(unsafe {
                (
                    surface.clone_unchecked(),
                    shell.clone_unchecked(),
                    xdg_surface.clone_unchecked(),
                )
            })) as *mut _);
            evlh.register(
                &popup,
                popup_implementation(),
                idata.clone(),
                Some(destroy_popup),
            );

            // register to self
            evlh.state()
                .get_mut(&idata.state_token)
                .known_popups
                .push(make_popup_handle(idata.compositor_token, &popup));

            // intial configure event
            let handle = make_popup_handle(idata.compositor_token, &popup);
            let mut user_idata = idata.idata.borrow_mut();
            let configure = (idata.implementation.new_popup)(evlh, &mut *user_idata, handle);
            send_popup_configure(idata.compositor_token, &popup, configure);
        },
        set_window_geometry: |_, idata, _, surface, x, y, width, height| {
            let ptr = surface.get_user_data();
            let &(ref surface, _) =
                unsafe { &*(ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };
            idata
                .compositor_token
                .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                    data.window_geometry = Some(Rectangle {
                        x,
                        y,
                        width,
                        height,
                    });
                })
                .expect(
                    "xdg_surface exists but surface has not shell_surface role?!",
                );
        },
        ack_configure: |_, idata, _, surface, serial| {
            let ptr = surface.get_user_data();
            let &(ref surface, ref shell) =
                unsafe { &*(ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };
            idata
                .compositor_token
                .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                    let mut found = false;
                    data.pending_configures.retain(|&s| {
                        if s == serial {
                            found = true;
                        }
                        s > serial
                    });
                    if !found {
                        // client responded to a non-existing configure
                        shell.post_error(
                            zxdg_shell_v6::Error::InvalidSurfaceState as u32,
                            format!("Wrong configure serial: {}", serial),
                        );
                    }
                    data.configured = true;
                })
                .expect(
                    "xdg_surface exists but surface has not shell_surface role?!",
                );
        },
    }
}

/*
 * xdg_toplevel
 */

pub type ShellSurfaceUserData = (
    wl_surface::WlSurface,
    zxdg_shell_v6::ZxdgShellV6,
    zxdg_surface_v6::ZxdgSurfaceV6,
);

fn destroy_toplevel(surface: &zxdg_toplevel_v6::ZxdgToplevelV6) {
    let ptr = surface.get_user_data();
    surface.set_user_data(::std::ptr::null_mut());
    // drop the PositionerState
    let data = unsafe { Box::from_raw(ptr as *mut ShellSurfaceUserData) };
    // explicit call to drop to not forget what we're doing there
    ::std::mem::drop(data);
}

// Utility functions allowing to factor out a lot of the upcoming logic
fn with_surface_toplevel_data<U, R, CID, SID, SD, F>(idata: &ShellSurfaceIData<U, R, CID, SID, SD>,
                                                     toplevel: &zxdg_toplevel_v6::ZxdgToplevelV6, f: F)
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: 'static,
    F: FnOnce(&mut ToplevelState),
{
    let ptr = toplevel.get_user_data();
    let &(ref surface, _, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
    idata
        .compositor_token
        .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| match data.pending_state {
            ShellSurfacePendingState::Toplevel(ref mut toplevel_data) => f(toplevel_data),
            _ => unreachable!(),
        })
        .expect(
            "xdg_toplevel exists but surface has not shell_surface role?!",
        );
}

fn xdg_handle_display_state_change<U, R, CID, SID, SD>(evlh: &mut EventLoopHandle,
                                                       idata: &ShellSurfaceIData<U, R, CID, SID, SD>,
                                                       toplevel: &zxdg_toplevel_v6::ZxdgToplevelV6,
                                                       maximized: Option<bool>, minimized: Option<bool>,
                                                       fullscreen: Option<bool>,
                                                       output: Option<&wl_output::WlOutput>)
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: 'static,
{
    let handle = make_toplevel_handle(idata.compositor_token, toplevel);
    // handler callback
    let mut user_idata = idata.idata.borrow_mut();
    let configure = (idata.implementation.change_display_state)(
        evlh,
        &mut *user_idata,
        handle,
        maximized,
        minimized,
        fullscreen,
        output,
    );
    // send the configure response to client
    send_toplevel_configure(idata.compositor_token, toplevel, configure);
}


pub fn send_toplevel_configure<U, R, ID>(token: CompositorToken<U, R, ID>,
                                         resource: &zxdg_toplevel_v6::ZxdgToplevelV6,
                                         configure: ToplevelConfigure)
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    ID: 'static,
{
    let &(ref surface, _, ref shell_surface) =
        unsafe { &*(resource.get_user_data() as *mut ShellSurfaceUserData) };
    let (w, h) = configure.size.unwrap_or((0, 0));
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
    resource.configure(w, h, states);
    shell_surface.configure(serial);
    // Add the configure as pending
    token
        .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| data.pending_configures.push(serial))
        .expect(
            "xdg_toplevel exists but surface has not shell_surface role?!",
        );
}

fn make_toplevel_handle<U, R, H, SD>(token: CompositorToken<U, R, H>,
                                     resource: &zxdg_toplevel_v6::ZxdgToplevelV6)
                                     -> super::ToplevelSurface<U, R, H, SD> {
    let ptr = resource.get_user_data();
    let &(ref wl_surface, _, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
    super::ToplevelSurface {
        wl_surface: unsafe { wl_surface.clone_unchecked() },
        shell_surface: super::SurfaceKind::XdgToplevel(unsafe { resource.clone_unchecked() }),
        token: token,
        _shell_data: ::std::marker::PhantomData,
    }
}

fn toplevel_implementation<U, R, CID, SID, SD>(
    )
    -> zxdg_toplevel_v6::Implementation<ShellSurfaceIData<U, R, CID, SID, SD>>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: 'static,
{
    zxdg_toplevel_v6::Implementation {
        destroy: |evlh, idata, _, toplevel| {
            let ptr = toplevel.get_user_data();
            let &(ref surface, _, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
            idata
                .compositor_token
                .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                    data.pending_state = ShellSurfacePendingState::None;
                    data.configured = false;
                })
                .expect(
                    "xdg_toplevel exists but surface has not shell_surface role?!",
                );
            // remove this surface from the known ones (as well as any leftover dead surface)
            evlh.state()
                .get_mut(&idata.state_token)
                .known_toplevels
                .retain(|other| {
                    other
                        .get_surface()
                        .map(|s| !s.equals(surface))
                        .unwrap_or(false)
                });
        },
        set_parent: |_, idata, _, toplevel, parent| {
            with_surface_toplevel_data(idata, toplevel, |toplevel_data| {
                toplevel_data.parent = parent.map(|toplevel_surface_parent| {
                    let parent_ptr = toplevel_surface_parent.get_user_data();
                    let &(ref parent_surface, _) =
                        unsafe { &*(parent_ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };
                    unsafe { parent_surface.clone_unchecked() }
                })
            });
        },
        set_title: |_, idata, _, toplevel, title| {
            with_surface_toplevel_data(idata, toplevel, |toplevel_data| {
                toplevel_data.title = title;
            });
        },
        set_app_id: |_, idata, _, toplevel, app_id| {
            with_surface_toplevel_data(idata, toplevel, |toplevel_data| {
                toplevel_data.app_id = app_id;
            });
        },
        show_window_menu: |evlh, idata, _, toplevel, seat, serial, x, y| {
            let handle = make_toplevel_handle(idata.compositor_token, toplevel);
            let mut user_idata = idata.idata.borrow_mut();
            (idata.implementation.show_window_menu)(evlh, &mut *user_idata, handle, seat, serial, x, y)
        },
        move_: |evlh, idata, _, toplevel, seat, serial| {
            let handle = make_toplevel_handle(idata.compositor_token, toplevel);
            let mut user_idata = idata.idata.borrow_mut();
            (idata.implementation.move_)(evlh, &mut *user_idata, handle, seat, serial)
        },
        resize: |evlh, idata, _, toplevel, seat, serial, edges| {
            let edges =
                zxdg_toplevel_v6::ResizeEdge::from_raw(edges).unwrap_or(zxdg_toplevel_v6::ResizeEdge::None);
            let handle = make_toplevel_handle(idata.compositor_token, toplevel);
            let mut user_idata = idata.idata.borrow_mut();
            (idata.implementation.resize)(evlh, &mut *user_idata, handle, seat, serial, edges)
        },
        set_max_size: |_, idata, _, toplevel, width, height| {
            with_surface_toplevel_data(idata, toplevel, |toplevel_data| {
                toplevel_data.max_size = (width, height);
            })
        },
        set_min_size: |_, idata, _, toplevel, width, height| {
            with_surface_toplevel_data(idata, toplevel, |toplevel_data| {
                toplevel_data.min_size = (width, height);
            })
        },
        set_maximized: |evlh, idata, _, toplevel| {
            xdg_handle_display_state_change(evlh, idata, toplevel, Some(true), None, None, None);
        },
        unset_maximized: |evlh, idata, _, toplevel| {
            xdg_handle_display_state_change(evlh, idata, toplevel, Some(false), None, None, None);
        },
        set_fullscreen: |evlh, idata, _, toplevel, seat| {
            xdg_handle_display_state_change(evlh, idata, toplevel, None, None, Some(true), seat);
        },
        unset_fullscreen: |evlh, idata, _, toplevel| {
            xdg_handle_display_state_change(evlh, idata, toplevel, None, None, Some(false), None);
        },
        set_minimized: |evlh, idata, _, toplevel| {
            xdg_handle_display_state_change(evlh, idata, toplevel, None, Some(true), None, None);
        },
    }
}

/*
 * xdg_popup
 */

fn destroy_popup(surface: &zxdg_popup_v6::ZxdgPopupV6) {
    let ptr = surface.get_user_data();
    surface.set_user_data(::std::ptr::null_mut());
    // drop the PositionerState
    let data = unsafe { Box::from_raw(ptr as *mut ShellSurfaceUserData) };
    // explicit call to drop to not forget what we're doing
    ::std::mem::drop(data);
}

pub(crate) fn send_popup_configure<U, R, ID>(token: CompositorToken<U, R, ID>,
                                             resource: &zxdg_popup_v6::ZxdgPopupV6,
                                             configure: PopupConfigure)
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    ID: 'static,
{
    let &(ref surface, _, ref shell_surface) =
        unsafe { &*(resource.get_user_data() as *mut ShellSurfaceUserData) };
    let (x, y) = configure.position;
    let (w, h) = configure.size;
    let serial = configure.serial;
    resource.configure(x, y, w, h);
    shell_surface.configure(serial);
    // Add the configure as pending
    token
        .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| data.pending_configures.push(serial))
        .expect(
            "xdg_toplevel exists but surface has not shell_surface role?!",
        );
}

fn make_popup_handle<U, R, H, SD>(token: CompositorToken<U, R, H>, resource: &zxdg_popup_v6::ZxdgPopupV6)
                                  -> super::PopupSurface<U, R, H, SD> {
    let ptr = resource.get_user_data();
    let &(ref wl_surface, _, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
    super::PopupSurface {
        wl_surface: unsafe { wl_surface.clone_unchecked() },
        shell_surface: super::SurfaceKind::XdgPopup(unsafe { resource.clone_unchecked() }),
        token: token,
        _shell_data: ::std::marker::PhantomData,
    }
}

fn popup_implementation<U, R, CID, SID, SD>(
    )
    -> zxdg_popup_v6::Implementation<ShellSurfaceIData<U, R, CID, SID, SD>>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: 'static,
{
    zxdg_popup_v6::Implementation {
        destroy: |evlh, idata, _, popup| {
            let ptr = popup.get_user_data();
            let &(ref surface, _, _) = unsafe {
                &*(ptr
                    as *mut (
                        wl_surface::WlSurface,
                        zxdg_shell_v6::ZxdgShellV6,
                        zxdg_surface_v6::ZxdgSurfaceV6,
                    ))
            };
            idata
                .compositor_token
                .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                    data.pending_state = ShellSurfacePendingState::None;
                    data.configured = false;
                })
                .expect(
                    "xdg_toplevel exists but surface has not shell_surface role?!",
                );
            // remove this surface from the known ones (as well as any leftover dead surface)
            evlh.state()
                .get_mut(&idata.state_token)
                .known_popups
                .retain(|other| {
                    other
                        .get_surface()
                        .map(|s| !s.equals(surface))
                        .unwrap_or(false)
                });
        },
        grab: |evlh, idata, _, popup, seat, serial| {
            let handle = make_popup_handle(idata.compositor_token, popup);
            let mut user_idata = idata.idata.borrow_mut();
            (idata.implementation.grab)(evlh, &mut *user_idata, handle, seat, serial)
        },
    }
}
