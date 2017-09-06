use super::{Handler as UserHandler, PopupConfigure, PopupState, PositionerState, ShellClient,
            ShellClientData, ShellHandler, ShellSurfacePendingState, ShellSurfaceRole, ToplevelConfigure,
            ToplevelState};

use compositor::{CompositorToken, Handler as CompositorHandler, Rectangle};
use compositor::roles::*;

use std::sync::Mutex;

use wayland_protocols::unstable::xdg_shell::server::{zxdg_positioner_v6 as xdg_positioner, zxdg_toplevel_v6};

use wayland_server::{Client, Destroy, EventLoopHandle, Resource};
use wayland_server::protocol::{wl_output, wl_seat, wl_shell, wl_shell_surface, wl_surface};

pub struct WlShellDestructor<SD> {
    _data: ::std::marker::PhantomData<SD>,
}

/*
 * wl_shell
 */

pub type ShellUserData<SD> = Mutex<(ShellClientData<SD>, Vec<wl_shell_surface::WlShellSurface>)>;

impl<SD> Destroy<wl_shell::WlShell> for WlShellDestructor<SD> {
    fn destroy(shell: &wl_shell::WlShell) {
        let ptr = shell.get_user_data();
        shell.set_user_data(::std::ptr::null_mut());
        let data = unsafe { Box::from_raw(ptr as *mut ShellUserData<SD>) };
        // explicitly call drop to not forget what we're doing here
        ::std::mem::drop(data);
    }
}

pub fn make_shell_client<SD>(resource: &wl_shell::WlShell) -> ShellClient<SD> {
    ShellClient {
        kind: super::ShellClientKind::Wl(unsafe { resource.clone_unchecked() }),
        _data: ::std::marker::PhantomData,
    }
}

impl<U, R, H, SH, SD> wl_shell::Handler for ShellHandler<U, R, H, SH, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
    SH: UserHandler<U, R, H, SD> + Send + 'static,
    SD: Send + 'static,
{
    fn get_shell_surface(&mut self, evlh: &mut EventLoopHandle, _: &Client, resource: &wl_shell::WlShell,
                         id: wl_shell_surface::WlShellSurface, surface: &wl_surface::WlSurface) {
        trace!(self.log, "Creating new wl_shell_surface.");
        let role_data = ShellSurfaceRole {
            pending_state: ShellSurfacePendingState::None,
            window_geometry: None,
            pending_configures: Vec::new(),
            configured: true,
        };
        if let Err(_) = self.token.give_role_with(surface, role_data) {
            resource.post_error(
                wl_shell::Error::Role as u32,
                "Surface already has a role.".into(),
            );
            return;
        }
        id.set_user_data(
            Box::into_raw(Box::new(unsafe { surface.clone_unchecked() })) as *mut _,
        );
        evlh.register_with_destructor::<_, Self, WlShellDestructor<SD>>(&id, self.my_id);

        // register ourselves to the wl_shell for ping handling
        let mutex = unsafe { &*(resource.get_user_data() as *mut ShellUserData<SD>) };
        let mut guard = mutex.lock().unwrap();
        if guard.1.len() == 0 && guard.0.pending_ping != 0 {
            // there is a pending ping that no surface could receive yet, send it
            // note this is not possible that it was received and then a wl_shell_surface was
            // destroyed, because wl_shell_surface has no destructor!
            id.ping(guard.0.pending_ping);
        }
        guard.1.push(id);
    }
}

server_declare_handler!(
    ShellHandler<U: [Send], R: [Role<ShellSurfaceRole>, Send], H:[CompositorHandler<U, R>, Send], SH:[UserHandler<U,R,H,SD>, Send], SD: [Send]>,
    wl_shell::Handler,
    wl_shell::WlShell
);

/*
 * wl_shell_surface
 */

pub type ShellSurfaceUserData = (wl_surface::WlSurface, wl_shell::WlShell);

impl<SD> Destroy<wl_shell_surface::WlShellSurface> for WlShellDestructor<SD> {
    fn destroy(shell_surface: &wl_shell_surface::WlShellSurface) {
        let ptr = shell_surface.get_user_data();
        shell_surface.set_user_data(::std::ptr::null_mut());
        // drop the WlSurface object
        let surface = unsafe { Box::from_raw(ptr as *mut ShellSurfaceUserData) };
        // explicitly call drop to not forget what we're doing here
        ::std::mem::drop(surface);
    }
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

impl<U, R, H, SH, SD> ShellHandler<U, R, H, SH, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
    SH: UserHandler<U, R, H, SD> + Send + 'static,
    SD: Send + 'static,
{
    fn wl_handle_display_state_change(&mut self, evlh: &mut EventLoopHandle,
                                      resource: &wl_shell_surface::WlShellSurface,
                                      maximized: Option<bool>, minimized: Option<bool>,
                                      fullscreen: Option<bool>, output: Option<&wl_output::WlOutput>) {
        let handle = make_toplevel_handle(self.token, resource);
        // handler callback
        let configure =
            self.handler
                .change_display_state(evlh, handle, maximized, minimized, fullscreen, output);
        // send the configure response to client
        let (w, h) = configure.size.unwrap_or((0, 0));
        resource.configure(wl_shell_surface::None, w, h);
    }

    fn wl_ensure_toplevel(&mut self, evlh: &mut EventLoopHandle,
                          resource: &wl_shell_surface::WlShellSurface) {
        let ptr = resource.get_user_data();
        let &(ref wl_surface, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
        // copy token to make borrow checker happy
        let token = self.token;
        let need_send = token
            .with_role_data::<ShellSurfaceRole, _, _>(wl_surface, |data| {
                match data.pending_state {
                    ShellSurfacePendingState::Toplevel(_) => {
                        return false;
                    }
                    ShellSurfacePendingState::Popup(_) => {
                        // this is no longer a popup, deregister it
                        self.known_popups.retain(|other| {
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
                return true;
            })
            .expect(
                "xdg_surface exists but surface has not shell_surface role?!",
            );
        // we need to notify about this new toplevel surface
        if need_send {
            let handle = make_toplevel_handle(self.token, resource);
            let configure = self.handler.new_toplevel(evlh, handle);
            send_toplevel_configure(resource, configure);
        }
    }
}

impl<U, R, H, SH, SD> wl_shell_surface::Handler for ShellHandler<U, R, H, SH, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
    SH: UserHandler<U, R, H, SD> + Send + 'static,
    SD: Send + 'static,
{
    fn pong(&mut self, evlh: &mut EventLoopHandle, _: &Client,
            resource: &wl_shell_surface::WlShellSurface, serial: u32) {
        let &(_, ref shell) = unsafe { &*(resource.get_user_data() as *mut ShellSurfaceUserData) };
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
            self.handler.client_pong(evlh, make_shell_client(shell));
        }
    }

    fn move_(&mut self, evlh: &mut EventLoopHandle, _: &Client,
             resource: &wl_shell_surface::WlShellSurface, seat: &wl_seat::WlSeat, serial: u32) {
        let handle = make_toplevel_handle(self.token, resource);
        self.handler.move_(evlh, handle, seat, serial);
    }

    fn resize(&mut self, evlh: &mut EventLoopHandle, _: &Client,
              resource: &wl_shell_surface::WlShellSurface, seat: &wl_seat::WlSeat, serial: u32,
              edges: wl_shell_surface::Resize) {
        let edges = zxdg_toplevel_v6::ResizeEdge::from_raw(edges.bits())
            .unwrap_or(zxdg_toplevel_v6::ResizeEdge::None);
        let handle = make_toplevel_handle(self.token, resource);
        self.handler.resize(evlh, handle, seat, serial, edges);
    }

    fn set_toplevel(&mut self, evlh: &mut EventLoopHandle, _: &Client,
                    resource: &wl_shell_surface::WlShellSurface) {
        self.wl_ensure_toplevel(evlh, resource);
        self.wl_handle_display_state_change(evlh, resource, Some(false), Some(false), Some(false), None)
    }

    fn set_transient(&mut self, evlh: &mut EventLoopHandle, _: &Client,
                     resource: &wl_shell_surface::WlShellSurface, parent: &wl_surface::WlSurface, _x: i32,
                     _y: i32, _flags: wl_shell_surface::Transient) {
        self.wl_ensure_toplevel(evlh, resource);
        // set the parent
        let ptr = resource.get_user_data();
        let &(ref wl_surface, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
        self.token
            .with_role_data::<ShellSurfaceRole, _, _>(wl_surface, |data| match data.pending_state {
                ShellSurfacePendingState::Toplevel(ref mut state) => {
                    state.parent = Some(unsafe { parent.clone_unchecked() });
                }
                _ => unreachable!(),
            })
            .unwrap();
        // set as regular surface
        self.wl_handle_display_state_change(evlh, resource, Some(false), Some(false), Some(false), None)
    }

    fn set_fullscreen(&mut self, evlh: &mut EventLoopHandle, _: &Client,
                      resource: &wl_shell_surface::WlShellSurface,
                      _method: wl_shell_surface::FullscreenMethod, _framerate: u32,
                      output: Option<&wl_output::WlOutput>) {
        self.wl_ensure_toplevel(evlh, resource);
        self.wl_handle_display_state_change(evlh, resource, Some(false), Some(false), Some(true), output)
    }

    fn set_popup(&mut self, evlh: &mut EventLoopHandle, _: &Client,
                 resource: &wl_shell_surface::WlShellSurface, seat: &wl_seat::WlSeat, serial: u32,
                 parent: &wl_surface::WlSurface, x: i32, y: i32, _: wl_shell_surface::Transient) {
        let ptr = resource.get_user_data();
        let &(ref wl_surface, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
        // we are reseting the popup state, so remove this surface from everywhere
        self.known_toplevels.retain(|other| {
            other
                .get_surface()
                .map(|s| !s.equals(wl_surface))
                .unwrap_or(false)
        });
        self.known_popups.retain(|other| {
            other
                .get_surface()
                .map(|s| !s.equals(wl_surface))
                .unwrap_or(false)
        });
        self.token
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
        let handle = make_popup_handle(self.token, resource);
        let configure = self.handler.new_popup(evlh, handle);
        send_popup_configure(resource, configure);
        self.handler
            .grab(evlh, make_popup_handle(self.token, resource), seat, serial);
    }

    fn set_maximized(&mut self, evlh: &mut EventLoopHandle, _: &Client,
                     resource: &wl_shell_surface::WlShellSurface, output: Option<&wl_output::WlOutput>) {
        self.wl_ensure_toplevel(evlh, resource);
        self.wl_handle_display_state_change(evlh, resource, Some(true), Some(false), Some(false), output)
    }

    fn set_title(&mut self, _: &mut EventLoopHandle, _: &Client,
                 resource: &wl_shell_surface::WlShellSurface, title: String) {
        let ptr = resource.get_user_data();
        let &(ref surface, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
        self.token
            .with_role_data(surface, |data| match data.pending_state {
                ShellSurfacePendingState::Toplevel(ref mut state) => {
                    state.title = title;
                }
                _ => {}
            })
            .expect("wl_shell_surface exists but wl_surface has wrong role?!");
    }

    fn set_class(&mut self, _: &mut EventLoopHandle, _: &Client,
                 resource: &wl_shell_surface::WlShellSurface, class_: String) {
        let ptr = resource.get_user_data();
        let &(ref surface, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
        self.token
            .with_role_data(surface, |data| match data.pending_state {
                ShellSurfacePendingState::Toplevel(ref mut state) => {
                    state.app_id = class_;
                }
                _ => {}
            })
            .expect("wl_shell_surface exists but wl_surface has wrong role?!");
    }
}

server_declare_handler!(
    ShellHandler<U: [Send], R: [Role<ShellSurfaceRole>, Send], H:[CompositorHandler<U, R>, Send], SH: [UserHandler<U,R,H,SD>, Send], SD: [Send]>,
    wl_shell_surface::Handler,
    wl_shell_surface::WlShellSurface
);
