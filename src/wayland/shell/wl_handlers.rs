use super::{make_shell_client_data, PopupConfigure, PopupState, PositionerState, ShellClient,
            ShellClientData, ShellSurfaceIData, ShellSurfacePendingState, ShellSurfaceRole,
            ToplevelConfigure, ToplevelState};
use std::sync::Mutex;
use utils::Rectangle;
use wayland::compositor::CompositorToken;
use wayland::compositor::roles::*;
use wayland_protocols::unstable::xdg_shell::server::{zxdg_positioner_v6 as xdg_positioner, zxdg_toplevel_v6};
use wayland_server::{Client, EventLoopHandle, Resource};
use wayland_server::protocol::{wl_output, wl_shell, wl_shell_surface, wl_surface};

pub(crate) fn wl_shell_bind<U, R, CID, SID, SD>(evlh: &mut EventLoopHandle,
                                                idata: &mut ShellSurfaceIData<U, R, CID, SID, SD>,
                                                _: &Client, shell: wl_shell::WlShell)
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: Default + 'static,
{
    shell.set_user_data(Box::into_raw(Box::new(Mutex::new((
        make_shell_client_data::<SD>(),
        Vec::<wl_shell_surface::WlShellSurface>::new(),
    )))) as *mut _);
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
 * wl_shell
 */

pub(crate) type ShellUserData<SD> = Mutex<(ShellClientData<SD>, Vec<wl_shell_surface::WlShellSurface>)>;

fn destroy_shell<SD>(shell: &wl_shell::WlShell) {
    let ptr = shell.get_user_data();
    shell.set_user_data(::std::ptr::null_mut());
    let data = unsafe { Box::from_raw(ptr as *mut ShellUserData<SD>) };
    // explicitly call drop to not forget what we're doing here
    ::std::mem::drop(data);
}

pub fn make_shell_client<SD>(resource: &wl_shell::WlShell) -> ShellClient<SD> {
    ShellClient {
        kind: super::ShellClientKind::Wl(unsafe { resource.clone_unchecked() }),
        _data: ::std::marker::PhantomData,
    }
}

fn shell_implementation<U, R, CID, SID, SD>(
    )
    -> wl_shell::Implementation<ShellSurfaceIData<U, R, CID, SID, SD>>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: 'static,
{
    wl_shell::Implementation {
        get_shell_surface: |evlh, idata, _, shell, shell_surface, surface| {
            let role_data = ShellSurfaceRole {
                pending_state: ShellSurfacePendingState::None,
                window_geometry: None,
                pending_configures: Vec::new(),
                configured: true,
            };
            if idata
                .compositor_token
                .give_role_with(surface, role_data)
                .is_err()
            {
                shell.post_error(
                    wl_shell::Error::Role as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }
            shell_surface
                .set_user_data(Box::into_raw(Box::new(unsafe { surface.clone_unchecked() })) as *mut _);
            evlh.register(
                &shell_surface,
                shell_surface_implementation(),
                idata.clone(),
                Some(destroy_shell_surface),
            );

            // register ourselves to the wl_shell for ping handling
            let mutex = unsafe { &*(shell.get_user_data() as *mut ShellUserData<SD>) };
            let mut guard = mutex.lock().unwrap();
            if guard.1.is_empty() && guard.0.pending_ping != 0 {
                // there is a pending ping that no surface could receive yet, send it
                // note this is not possible that it was received and then a wl_shell_surface was
                // destroyed, because wl_shell_surface has no destructor!
                shell_surface.ping(guard.0.pending_ping);
            }
            guard.1.push(shell_surface);
        },
    }
}

/*
 * wl_shell_surface
 */

pub type ShellSurfaceUserData = (wl_surface::WlSurface, wl_shell::WlShell);

fn destroy_shell_surface(shell_surface: &wl_shell_surface::WlShellSurface) {
    let ptr = shell_surface.get_user_data();
    shell_surface.set_user_data(::std::ptr::null_mut());
    // drop the WlSurface object
    let surface = unsafe { Box::from_raw(ptr as *mut ShellSurfaceUserData) };
    // explicitly call drop to not forget what we're doing here
    ::std::mem::drop(surface);
}

fn make_toplevel_handle<U, R, H, SD>(token: CompositorToken<U, R, H>,
                                     resource: &wl_shell_surface::WlShellSurface)
                                     -> super::ToplevelSurface<U, R, H, SD> {
    let ptr = resource.get_user_data();
    let &(ref wl_surface, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
    super::ToplevelSurface {
        wl_surface: unsafe { wl_surface.clone_unchecked() },
        shell_surface: super::SurfaceKind::Wl(unsafe { resource.clone_unchecked() }),
        token: token,
        _shell_data: ::std::marker::PhantomData,
    }
}

fn make_popup_handle<U, R, H, SD>(token: CompositorToken<U, R, H>,
                                  resource: &wl_shell_surface::WlShellSurface)
                                  -> super::PopupSurface<U, R, H, SD> {
    let ptr = resource.get_user_data();
    let &(ref wl_surface, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
    super::PopupSurface {
        wl_surface: unsafe { wl_surface.clone_unchecked() },
        shell_surface: super::SurfaceKind::Wl(unsafe { resource.clone_unchecked() }),
        token: token,
        _shell_data: ::std::marker::PhantomData,
    }
}

pub fn send_toplevel_configure(resource: &wl_shell_surface::WlShellSurface, configure: ToplevelConfigure) {
    let (w, h) = configure.size.unwrap_or((0, 0));
    resource.configure(wl_shell_surface::Resize::empty(), w, h);
}

pub fn send_popup_configure(resource: &wl_shell_surface::WlShellSurface, configure: PopupConfigure) {
    let (w, h) = configure.size;
    resource.configure(wl_shell_surface::Resize::empty(), w, h);
}

fn wl_handle_display_state_change<U, R, CID, SID, SD>(evlh: &mut EventLoopHandle,
                                                      idata: &ShellSurfaceIData<U, R, CID, SID, SD>,
                                                      shell_surface: &wl_shell_surface::WlShellSurface,
                                                      maximized: Option<bool>, minimized: Option<bool>,
                                                      fullscreen: Option<bool>,
                                                      output: Option<&wl_output::WlOutput>) {
    let handle = make_toplevel_handle(idata.compositor_token, shell_surface);
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
    let (w, h) = configure.size.unwrap_or((0, 0));
    shell_surface.configure(wl_shell_surface::None, w, h);
}

fn wl_set_parent<U, R, CID, SID, SD>(idata: &ShellSurfaceIData<U, R, CID, SID, SD>,
                                     shell_surface: &wl_shell_surface::WlShellSurface,
                                     parent: Option<wl_surface::WlSurface>)
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: 'static,
{
    let ptr = shell_surface.get_user_data();
    let &(ref wl_surface, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
    idata
        .compositor_token
        .with_role_data::<ShellSurfaceRole, _, _>(wl_surface, |data| match data.pending_state {
            ShellSurfacePendingState::Toplevel(ref mut state) => {
                state.parent = parent;
            }
            _ => unreachable!(),
        })
        .unwrap();
}

fn wl_ensure_toplevel<U, R, CID, SID, SD>(evlh: &mut EventLoopHandle,
                                          idata: &ShellSurfaceIData<U, R, CID, SID, SD>,
                                          shell_surface: &wl_shell_surface::WlShellSurface)
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: 'static,
{
    let ptr = shell_surface.get_user_data();
    let &(ref wl_surface, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
    // copy token to make borrow checker happy
    let token = idata.compositor_token;
    let need_send = token
        .with_role_data::<ShellSurfaceRole, _, _>(wl_surface, |data| {
            match data.pending_state {
                ShellSurfacePendingState::Toplevel(_) => {
                    return false;
                }
                ShellSurfacePendingState::Popup(_) => {
                    // this is no longer a popup, deregister it
                    evlh.state()
                        .get_mut(&idata.state_token)
                        .known_popups
                        .retain(|other| {
                            other
                                .get_surface()
                                .map(|s| !s.equals(wl_surface))
                                .unwrap_or(false)
                        });
                }
                ShellSurfacePendingState::None => {}
            }
            // This was not previously toplevel, need to make it toplevel
            data.pending_state = ShellSurfacePendingState::Toplevel(ToplevelState {
                parent: None,
                title: String::new(),
                app_id: String::new(),
                min_size: (0, 0),
                max_size: (0, 0),
            });
            true
        })
        .expect("xdg_surface exists but surface has not shell_surface role?!");
    // we need to notify about this new toplevel surface
    if need_send {
        evlh.state()
            .get_mut(&idata.state_token)
            .known_toplevels
            .push(make_toplevel_handle(idata.compositor_token, shell_surface));
        let handle = make_toplevel_handle(idata.compositor_token, shell_surface);
        let mut user_idata = idata.idata.borrow_mut();
        let configure = (idata.implementation.new_toplevel)(evlh, &mut *user_idata, handle);
        send_toplevel_configure(shell_surface, configure);
    }
}

fn shell_surface_implementation<U, R, CID, SID, SD>(
    )
    -> wl_shell_surface::Implementation<ShellSurfaceIData<U, R, CID, SID, SD>>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    CID: 'static,
    SID: 'static,
    SD: 'static,
{
    wl_shell_surface::Implementation {
        pong: |evlh, idata, _, shell_surface, serial| {
            let &(_, ref shell) = unsafe { &*(shell_surface.get_user_data() as *mut ShellSurfaceUserData) };
            let valid = {
                let mutex = unsafe { &*(shell.get_user_data() as *mut ShellUserData<SD>) };
                let mut guard = mutex.lock().unwrap();
                if guard.0.pending_ping == serial {
                    guard.0.pending_ping = 0;
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
        move_: |evlh, idata, _, shell_surface, seat, serial| {
            let handle = make_toplevel_handle(idata.compositor_token, shell_surface);
            let mut user_idata = idata.idata.borrow_mut();
            (idata.implementation.move_)(evlh, &mut *user_idata, handle, seat, serial);
        },
        resize: |evlh, idata, _, shell_surface, seat, serial, edges| {
            let edges = zxdg_toplevel_v6::ResizeEdge::from_raw(edges.bits())
                .unwrap_or(zxdg_toplevel_v6::ResizeEdge::None);
            let handle = make_toplevel_handle(idata.compositor_token, shell_surface);
            let mut user_idata = idata.idata.borrow_mut();
            (idata.implementation.resize)(evlh, &mut *user_idata, handle, seat, serial, edges);
        },
        set_toplevel: |evlh, idata, _, shell_surface| {
            wl_ensure_toplevel(evlh, idata, shell_surface);
            wl_set_parent(idata, shell_surface, None);
            wl_handle_display_state_change(
                evlh,
                idata,
                shell_surface,
                Some(false),
                Some(false),
                Some(false),
                None,
            )
        },
        set_transient: |evlh, idata, _, shell_surface, parent, _, _, _| {
            wl_ensure_toplevel(evlh, idata, shell_surface);
            wl_set_parent(
                idata,
                shell_surface,
                Some(unsafe { parent.clone_unchecked() }),
            );
            wl_handle_display_state_change(
                evlh,
                idata,
                shell_surface,
                Some(false),
                Some(false),
                Some(false),
                None,
            )
        },
        set_fullscreen: |evlh, idata, _, shell_surface, _, _, output| {
            wl_ensure_toplevel(evlh, idata, shell_surface);
            wl_set_parent(idata, shell_surface, None);
            wl_handle_display_state_change(
                evlh,
                idata,
                shell_surface,
                Some(false),
                Some(false),
                Some(true),
                output,
            )
        },
        set_popup: |evlh, idata, _, shell_surface, seat, serial, parent, x, y, _| {
            let ptr = shell_surface.get_user_data();
            let &(ref wl_surface, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
            // we are reseting the popup state, so remove this surface from everywhere
            evlh.state()
                .get_mut(&idata.state_token)
                .known_toplevels
                .retain(|other| {
                    other
                        .get_surface()
                        .map(|s| !s.equals(wl_surface))
                        .unwrap_or(false)
                });
            evlh.state()
                .get_mut(&idata.state_token)
                .known_popups
                .retain(|other| {
                    other
                        .get_surface()
                        .map(|s| !s.equals(wl_surface))
                        .unwrap_or(false)
                });
            idata
                .compositor_token
                .with_role_data(wl_surface, |data| {
                    data.pending_state = ShellSurfacePendingState::Popup(PopupState {
                        parent: unsafe { parent.clone_unchecked() },
                        positioner: PositionerState {
                            rect_size: (1, 1),
                            anchor_rect: Rectangle {
                                x,
                                y,
                                width: 1,
                                height: 1,
                            },
                            anchor_edges: xdg_positioner::Anchor::empty(),
                            gravity: xdg_positioner::Gravity::empty(),
                            constraint_adjustment: xdg_positioner::ConstraintAdjustment::empty(),
                            offset: (0, 0),
                        },
                    });
                })
                .expect("wl_shell_surface exists but wl_surface has wrong role?!");

            // notify the handler about this new popup
            evlh.state()
                .get_mut(&idata.state_token)
                .known_popups
                .push(make_popup_handle(idata.compositor_token, shell_surface));
            let handle = make_popup_handle(idata.compositor_token, shell_surface);
            let mut user_idata = idata.idata.borrow_mut();
            let configure = (idata.implementation.new_popup)(evlh, &mut *user_idata, handle);
            send_popup_configure(shell_surface, configure);
            (idata.implementation.grab)(
                evlh,
                &mut *user_idata,
                make_popup_handle(idata.compositor_token, shell_surface),
                seat,
                serial,
            );
        },
        set_maximized: |evlh, idata, _, shell_surface, output| {
            wl_ensure_toplevel(evlh, idata, shell_surface);
            wl_set_parent(idata, shell_surface, None);
            wl_handle_display_state_change(
                evlh,
                idata,
                shell_surface,
                Some(true),
                Some(false),
                Some(false),
                output,
            )
        },
        set_title: |_, idata, _, shell_surface, title| {
            let ptr = shell_surface.get_user_data();
            let &(ref surface, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
            idata
                .compositor_token
                .with_role_data(surface, |data| {
                    if let ShellSurfacePendingState::Toplevel(ref mut state) = data.pending_state {
                        state.title = title;
                    }
                })
                .expect("wl_shell_surface exists but wl_surface has wrong role?!");
        },
        set_class: |_, idata, _, shell_surface, class| {
            let ptr = shell_surface.get_user_data();
            let &(ref surface, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
            idata
                .compositor_token
                .with_role_data(surface, |data| match data.pending_state {
                    ShellSurfacePendingState::Toplevel(ref mut state) => {
                        state.app_id = class;
                    }
                    _ => {}
                })
                .expect("wl_shell_surface exists but wl_surface has wrong role?!");
        },
    }
}
