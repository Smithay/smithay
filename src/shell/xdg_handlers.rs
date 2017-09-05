use super::{Handler as UserHandler, PopupConfigure, PopupState, PositionerState, ShellClient,
            ShellClientData, ShellHandler, ShellSurfacePendingState, ShellSurfaceRole, ToplevelConfigure,
            ToplevelState};

use compositor::{CompositorToken, Handler as CompositorHandler, Rectangle};
use compositor::roles::*;

use std::sync::Mutex;

use wayland_protocols::unstable::xdg_shell::server::{zxdg_popup_v6, zxdg_positioner_v6, zxdg_shell_v6,
                                                     zxdg_surface_v6, zxdg_toplevel_v6};
use wayland_server::{Client, Destroy, EventLoopHandle, Init, Resource};
use wayland_server::protocol::{wl_output, wl_seat, wl_surface};

pub struct XdgShellDestructor<SD> {
    _data: ::std::marker::PhantomData<SD>,
}

/*
 * xdg_shell
 */

pub type ShellUserData<SD> = Mutex<ShellClientData<SD>>;

impl<SD> Destroy<zxdg_shell_v6::ZxdgShellV6> for XdgShellDestructor<SD> {
    fn destroy(shell: &zxdg_shell_v6::ZxdgShellV6) {
        let ptr = shell.get_user_data();
        shell.set_user_data(::std::ptr::null_mut());
        let data = unsafe { Box::from_raw(ptr as *mut ShellUserData<SD>) };
    }
}

pub fn make_shell_client<SD>(resource: &zxdg_shell_v6::ZxdgShellV6) -> ShellClient<SD> {
    ShellClient {
        kind: super::ShellClientKind::Xdg(unsafe { resource.clone_unchecked() }),
        _data: ::std::marker::PhantomData,
    }
}

impl<U, R, H, SH, SD> zxdg_shell_v6::Handler for ShellHandler<U, R, H, SH, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
    SH: UserHandler<U, R, H, SD> + Send + 'static,
    SD: Send + 'static,
{
    fn destroy(&mut self, evqh: &mut EventLoopHandle, client: &Client,
               resource: &zxdg_shell_v6::ZxdgShellV6) {
    }
    fn create_positioner(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                         resource: &zxdg_shell_v6::ZxdgShellV6, id: zxdg_positioner_v6::ZxdgPositionerV6) {
        trace!(self.log, "Creating new xdg_positioner.");
        id.set_user_data(Box::into_raw(Box::new(PositionerState {
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
        })) as *mut _);
        evqh.register_with_destructor::<_, Self, XdgShellDestructor<SD>>(&id, self.my_id);
    }
    fn get_xdg_surface(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                       resource: &zxdg_shell_v6::ZxdgShellV6, id: zxdg_surface_v6::ZxdgSurfaceV6,
                       surface: &wl_surface::WlSurface) {
        trace!(self.log, "Creating new wl_shell_surface.");
        let role_data = ShellSurfaceRole {
            pending_state: ShellSurfacePendingState::None,
            window_geometry: None,
            pending_configures: Vec::new(),
            configured: false,
        };
        if let Err(_) = self.token.give_role_with(surface, role_data) {
            resource.post_error(
                zxdg_shell_v6::Error::Role as u32,
                "Surface already has a role.".into(),
            );
            return;
        }
        id.set_user_data(
            Box::into_raw(Box::new((unsafe { surface.clone_unchecked() }, unsafe {
                resource.clone_unchecked()
            }))) as *mut _,
        );
        evqh.register_with_destructor::<_, Self, XdgShellDestructor<SD>>(&id, self.my_id);
    }

    fn pong(&mut self, evqh: &mut EventLoopHandle, client: &Client, resource: &zxdg_shell_v6::ZxdgShellV6,
            serial: u32) {
        let valid = {
            let mutex = unsafe { &*(resource.get_user_data() as *mut ShellUserData<SD>) };
            let mut guard = mutex.lock().unwrap();
            if guard.pending_ping == serial {
                guard.pending_ping = 0;
                true
            } else {
                false
            }
        };
        if valid {
            self.handler.client_pong(evqh, make_shell_client(resource));
        }
    }
}

server_declare_handler!(
    ShellHandler<U: [Send], R: [Role<ShellSurfaceRole>, Send], H:[CompositorHandler<U, R>, Send], SH: [UserHandler<U,R,H,SD>, Send], SD: [Send]>,
    zxdg_shell_v6::Handler,
    zxdg_shell_v6::ZxdgShellV6
);

/*
 * xdg_positioner
 */

impl<SD> Destroy<zxdg_positioner_v6::ZxdgPositionerV6> for XdgShellDestructor<SD> {
    fn destroy(positioner: &zxdg_positioner_v6::ZxdgPositionerV6) {
        let ptr = positioner.get_user_data();
        positioner.set_user_data(::std::ptr::null_mut());
        // drop the PositionerState
        let surface = unsafe { Box::from_raw(ptr as *mut PositionerState) };
    }
}

impl<U, R, H, SH, SD> zxdg_positioner_v6::Handler for ShellHandler<U, R, H, SH, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
    SH: UserHandler<U, R, H, SD> + Send + 'static,
    SD: Send + 'static,
{
    fn destroy(&mut self, evqh: &mut EventLoopHandle, client: &Client,
               resource: &zxdg_positioner_v6::ZxdgPositionerV6) {
    }

    fn set_size(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                resource: &zxdg_positioner_v6::ZxdgPositionerV6, width: i32, height: i32) {
        if width < 1 || height < 1 {
            resource.post_error(
                zxdg_positioner_v6::Error::InvalidInput as u32,
                "Invalid size for positioner.".into(),
            );
        } else {
            let ptr = resource.get_user_data();
            let state = unsafe { &mut *(ptr as *mut PositionerState) };
            state.rect_size = (width, height);
        }
    }

    fn set_anchor_rect(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                       resource: &zxdg_positioner_v6::ZxdgPositionerV6, x: i32, y: i32, width: i32,
                       height: i32) {
        if width < 1 || height < 1 {
            resource.post_error(
                zxdg_positioner_v6::Error::InvalidInput as u32,
                "Invalid size for positioner's anchor rectangle.".into(),
            );
        } else {
            let ptr = resource.get_user_data();
            let state = unsafe { &mut *(ptr as *mut PositionerState) };
            state.anchor_rect = Rectangle {
                x,
                y,
                width,
                height,
            };
        }
    }

    fn set_anchor(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                  resource: &zxdg_positioner_v6::ZxdgPositionerV6, anchor: zxdg_positioner_v6::Anchor) {
        use self::zxdg_positioner_v6::{AnchorBottom, AnchorLeft, AnchorRight, AnchorTop};
        if anchor.contains(AnchorLeft | AnchorRight) || anchor.contains(AnchorTop | AnchorBottom) {
            resource.post_error(
                zxdg_positioner_v6::Error::InvalidInput as u32,
                "Invalid anchor for positioner.".into(),
            );
        } else {
            let ptr = resource.get_user_data();
            let state = unsafe { &mut *(ptr as *mut PositionerState) };
            state.anchor_edges = anchor;
        }
    }

    fn set_gravity(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                   resource: &zxdg_positioner_v6::ZxdgPositionerV6, gravity: zxdg_positioner_v6::Gravity) {
        use self::zxdg_positioner_v6::{GravityBottom, GravityLeft, GravityRight, GravityTop};
        if gravity.contains(GravityLeft | GravityRight) || gravity.contains(GravityTop | GravityBottom) {
            resource.post_error(
                zxdg_positioner_v6::Error::InvalidInput as u32,
                "Invalid gravity for positioner.".into(),
            );
        } else {
            let ptr = resource.get_user_data();
            let state = unsafe { &mut *(ptr as *mut PositionerState) };
            state.gravity = gravity;
        }
    }

    fn set_constraint_adjustment(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                                 resource: &zxdg_positioner_v6::ZxdgPositionerV6,
                                 constraint_adjustment: u32) {
        let constraint_adjustment =
            zxdg_positioner_v6::ConstraintAdjustment::from_bits_truncate(constraint_adjustment);
        let ptr = resource.get_user_data();
        let state = unsafe { &mut *(ptr as *mut PositionerState) };
        state.constraint_adjustment = constraint_adjustment;
    }

    fn set_offset(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                  resource: &zxdg_positioner_v6::ZxdgPositionerV6, x: i32, y: i32) {
        let ptr = resource.get_user_data();
        let state = unsafe { &mut *(ptr as *mut PositionerState) };
        state.offset = (x, y);
    }
}

server_declare_handler!(
    ShellHandler<U: [Send], R: [Role<ShellSurfaceRole>, Send], H:[CompositorHandler<U, R>, Send], SH: [UserHandler<U,R,H,SD>, Send], SD: [Send]>,
    zxdg_positioner_v6::Handler,
    zxdg_positioner_v6::ZxdgPositionerV6
);

/*
 * xdg_surface
 */

impl<SD> Destroy<zxdg_surface_v6::ZxdgSurfaceV6> for XdgShellDestructor<SD> {
    fn destroy(surface: &zxdg_surface_v6::ZxdgSurfaceV6) {
        let ptr = surface.get_user_data();
        surface.set_user_data(::std::ptr::null_mut());
        // drop the PositionerState
        let data = unsafe {
            Box::from_raw(
                ptr as *mut (zxdg_surface_v6::ZxdgSurfaceV6, zxdg_shell_v6::ZxdgShellV6),
            )
        };
    }
}

impl<U, R, H, SH, SD> zxdg_surface_v6::Handler for ShellHandler<U, R, H, SH, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
    SH: UserHandler<U, R, H, SD> + Send + 'static,
    SD: Send + 'static,
{
    fn destroy(&mut self, evqh: &mut EventLoopHandle, client: &Client,
               resource: &zxdg_surface_v6::ZxdgSurfaceV6) {
        let ptr = resource.get_user_data();
        let &(ref surface, ref shell) =
            unsafe { &*(ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };
        self.token
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
    }

    fn get_toplevel(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                    resource: &zxdg_surface_v6::ZxdgSurfaceV6, id: zxdg_toplevel_v6::ZxdgToplevelV6) {
        let ptr = resource.get_user_data();
        let &(ref surface, ref shell) =
            unsafe { &*(ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };
        self.token
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

        id.set_user_data(Box::into_raw(Box::new(unsafe {
            (
                surface.clone_unchecked(),
                shell.clone_unchecked(),
                resource.clone_unchecked(),
            )
        })) as *mut _);
        evqh.register_with_destructor::<_, Self, XdgShellDestructor<SD>>(&id, self.my_id);

        // register to self
        self.known_toplevels
            .push(make_toplevel_handle(self.token, &id));

        // intial configure event
        let handle = make_toplevel_handle(self.token, &id);
        let configure = self.handler.new_toplevel(evqh, handle);
        send_toplevel_configure(self.token, &id, configure);
    }

    fn get_popup(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                 resource: &zxdg_surface_v6::ZxdgSurfaceV6, id: zxdg_popup_v6::ZxdgPopupV6,
                 parent: &zxdg_surface_v6::ZxdgSurfaceV6,
                 positioner: &zxdg_positioner_v6::ZxdgPositionerV6) {
        let ptr = resource.get_user_data();
        let &(ref surface, ref shell) =
            unsafe { &*(ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };

        let positioner_data = unsafe { &*(positioner.get_user_data() as *const PositionerState) };

        let parent_ptr = parent.get_user_data();
        let &(ref parent_surface, _) =
            unsafe { &*(ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };

        self.token
            .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                data.pending_state = ShellSurfacePendingState::Popup(PopupState {
                    parent: unsafe { parent_surface.clone_unchecked() },
                    positioner: positioner_data.clone(),
                });
            })
            .expect(
                "xdg_surface exists but surface has not shell_surface role?!",
            );

        id.set_user_data(Box::into_raw(Box::new(unsafe {
            (
                surface.clone_unchecked(),
                shell.clone_unchecked(),
                resource.clone_unchecked(),
            )
        })) as *mut _);
        evqh.register_with_destructor::<_, Self, XdgShellDestructor<SD>>(&id, self.my_id);

        // register to self
        self.known_popups.push(make_popup_handle(self.token, &id));

        // intial configure event
        let handle = make_popup_handle(self.token, &id);
        let configure = self.handler.new_popup(evqh, handle);
        send_popup_configure(self.token, &id, configure);
    }

    fn set_window_geometry(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                           resource: &zxdg_surface_v6::ZxdgSurfaceV6, x: i32, y: i32, width: i32,
                           height: i32) {
        let ptr = resource.get_user_data();
        let &(ref surface, _) =
            unsafe { &*(ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };
        self.token
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
    }

    fn ack_configure(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                     resource: &zxdg_surface_v6::ZxdgSurfaceV6, serial: u32) {
        let ptr = resource.get_user_data();
        let &(ref surface, ref shell) =
            unsafe { &*(ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };
        self.token
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
    }
}

server_declare_handler!(
    ShellHandler<U: [Send], R: [Role<ShellSurfaceRole>, Send], H:[CompositorHandler<U, R>, Send], SH: [UserHandler<U,R,H,SD>, Send], SD: [Send]>,
    zxdg_surface_v6::Handler,
    zxdg_surface_v6::ZxdgSurfaceV6
);

/*
 * xdg_toplevel
 */

pub type ShellSurfaceUserData = (
    wl_surface::WlSurface,
    zxdg_shell_v6::ZxdgShellV6,
    zxdg_surface_v6::ZxdgSurfaceV6,
);

impl<SD> Destroy<zxdg_toplevel_v6::ZxdgToplevelV6> for XdgShellDestructor<SD> {
    fn destroy(surface: &zxdg_toplevel_v6::ZxdgToplevelV6) {
        let ptr = surface.get_user_data();
        surface.set_user_data(::std::ptr::null_mut());
        // drop the PositionerState
        let data = unsafe { Box::from_raw(ptr as *mut ShellSurfaceUserData) };
    }
}

impl<U, R, H, SH, SD> ShellHandler<U, R, H, SH, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
    SH: UserHandler<U, R, H, SD> + Send + 'static,
    SD: Send + 'static,
{
    // Utility function allowing to factor out a lot of the upcoming logic
    fn with_surface_toplevel_data<F>(&self, resource: &zxdg_toplevel_v6::ZxdgToplevelV6, f: F)
    where
        F: FnOnce(&mut ToplevelState),
    {
        let ptr = resource.get_user_data();
        let &(ref surface, _, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
        self.token
            .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| match data.pending_state {
                ShellSurfacePendingState::Toplevel(ref mut toplevel_data) => f(toplevel_data),
                _ => unreachable!(),
            })
            .expect(
                "xdg_toplevel exists but surface has not shell_surface role?!",
            );
    }

    fn xdg_handle_display_state_change(&mut self, evqh: &mut EventLoopHandle,
                                       resource: &zxdg_toplevel_v6::ZxdgToplevelV6,
                                       maximized: Option<bool>, minimized: Option<bool>,
                                       fullscreen: Option<bool>, output: Option<&wl_output::WlOutput>) {
        let handle = make_toplevel_handle(self.token, resource);
        // handler callback
        let configure =
            self.handler
                .change_display_state(evqh, handle, maximized, minimized, fullscreen, output);
        // send the configure response to client
        send_toplevel_configure(self.token, resource, configure);
    }
}

pub fn send_toplevel_configure<U, R, H>(token: CompositorToken<U, R, H>,
                                        resource: &zxdg_toplevel_v6::ZxdgToplevelV6,
                                        configure: ToplevelConfigure)
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
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

impl<U, R, H, SH, SD> zxdg_toplevel_v6::Handler for ShellHandler<U, R, H, SH, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
    SH: UserHandler<U, R, H, SD> + Send + 'static,
    SD: Send + 'static,
{
    fn destroy(&mut self, evqh: &mut EventLoopHandle, client: &Client,
               resource: &zxdg_toplevel_v6::ZxdgToplevelV6) {
        let ptr = resource.get_user_data();
        let &(ref surface, _, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
        self.token
            .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                data.pending_state = ShellSurfacePendingState::None;
                data.configured = false;
            })
            .expect(
                "xdg_toplevel exists but surface has not shell_surface role?!",
            );
        // remove this surface from the known ones (as well as any leftover dead surface)
        self.known_toplevels.retain(|other| {
            other
                .get_surface()
                .map(|s| !s.equals(surface))
                .unwrap_or(false)
        });
    }

    fn set_parent(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                  resource: &zxdg_toplevel_v6::ZxdgToplevelV6,
                  parent: Option<&zxdg_toplevel_v6::ZxdgToplevelV6>) {
        self.with_surface_toplevel_data(resource, |toplevel_data| {
            toplevel_data.parent = parent.map(|toplevel_surface_parent| {
                let parent_ptr = toplevel_surface_parent.get_user_data();
                let &(ref parent_surface, _) =
                    unsafe { &*(parent_ptr as *mut (wl_surface::WlSurface, zxdg_shell_v6::ZxdgShellV6)) };
                unsafe { parent_surface.clone_unchecked() }
            })
        });
    }

    fn set_title(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                 resource: &zxdg_toplevel_v6::ZxdgToplevelV6, title: String) {
        self.with_surface_toplevel_data(resource, |toplevel_data| { toplevel_data.title = title; });
    }

    fn set_app_id(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                  resource: &zxdg_toplevel_v6::ZxdgToplevelV6, app_id: String) {
        self.with_surface_toplevel_data(resource, |toplevel_data| { toplevel_data.app_id = app_id; });
    }

    fn show_window_menu(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                        resource: &zxdg_toplevel_v6::ZxdgToplevelV6, seat: &wl_seat::WlSeat, serial: u32,
                        x: i32, y: i32) {
        let handle = make_toplevel_handle(self.token, resource);
        self.handler
            .show_window_menu(evqh, handle, seat, serial, x, y);
    }

    fn move_(&mut self, evqh: &mut EventLoopHandle, client: &Client,
             resource: &zxdg_toplevel_v6::ZxdgToplevelV6, seat: &wl_seat::WlSeat, serial: u32) {
        let handle = make_toplevel_handle(self.token, resource);
        self.handler.move_(evqh, handle, seat, serial);
    }

    fn resize(&mut self, evqh: &mut EventLoopHandle, client: &Client,
              resource: &zxdg_toplevel_v6::ZxdgToplevelV6, seat: &wl_seat::WlSeat, serial: u32, edges: u32) {
        let edges =
            zxdg_toplevel_v6::ResizeEdge::from_raw(edges).unwrap_or(zxdg_toplevel_v6::ResizeEdge::None);
        let handle = make_toplevel_handle(self.token, resource);
        self.handler.resize(evqh, handle, seat, serial, edges);
    }

    fn set_max_size(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                    resource: &zxdg_toplevel_v6::ZxdgToplevelV6, width: i32, height: i32) {
        self.with_surface_toplevel_data(resource, |toplevel_data| {
            toplevel_data.max_size = (width, height);
        });
    }

    fn set_min_size(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                    resource: &zxdg_toplevel_v6::ZxdgToplevelV6, width: i32, height: i32) {
        self.with_surface_toplevel_data(resource, |toplevel_data| {
            toplevel_data.min_size = (width, height);
        });
    }

    fn set_maximized(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                     resource: &zxdg_toplevel_v6::ZxdgToplevelV6) {
        self.xdg_handle_display_state_change(evqh, resource, Some(true), None, None, None);
    }

    fn unset_maximized(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                       resource: &zxdg_toplevel_v6::ZxdgToplevelV6) {
        self.xdg_handle_display_state_change(evqh, resource, Some(false), None, None, None);
    }

    fn set_fullscreen(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                      resource: &zxdg_toplevel_v6::ZxdgToplevelV6, output: Option<&wl_output::WlOutput>) {
        self.xdg_handle_display_state_change(evqh, resource, None, None, Some(true), output);
    }

    fn unset_fullscreen(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                        resource: &zxdg_toplevel_v6::ZxdgToplevelV6) {
        self.xdg_handle_display_state_change(evqh, resource, None, None, Some(false), None);
    }

    fn set_minimized(&mut self, evqh: &mut EventLoopHandle, client: &Client,
                     resource: &zxdg_toplevel_v6::ZxdgToplevelV6) {
        self.xdg_handle_display_state_change(evqh, resource, None, Some(true), None, None);
    }
}

server_declare_handler!(
    ShellHandler<U: [Send], R: [Role<ShellSurfaceRole>, Send], H:[CompositorHandler<U, R>, Send], SH: [UserHandler<U,R,H,SD>, Send], SD: [Send]>,
    zxdg_toplevel_v6::Handler,
    zxdg_toplevel_v6::ZxdgToplevelV6
);

/*
 * xdg_popup
 */



impl<SD> Destroy<zxdg_popup_v6::ZxdgPopupV6> for XdgShellDestructor<SD> {
    fn destroy(surface: &zxdg_popup_v6::ZxdgPopupV6) {
        let ptr = surface.get_user_data();
        surface.set_user_data(::std::ptr::null_mut());
        // drop the PositionerState
        let data = unsafe { Box::from_raw(ptr as *mut ShellSurfaceUserData) };
    }
}

pub fn send_popup_configure<U, R, H>(token: CompositorToken<U, R, H>, resource: &zxdg_popup_v6::ZxdgPopupV6,
                                     configure: PopupConfigure)
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
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

impl<U, R, H, SH, SD> zxdg_popup_v6::Handler for ShellHandler<U, R, H, SH, SD>
where
    U: Send + 'static,
    R: Role<ShellSurfaceRole> + Send + 'static,
    H: CompositorHandler<U, R> + Send + 'static,
    SH: UserHandler<U, R, H, SD> + Send + 'static,
    SD: Send + 'static,
{
    fn destroy(&mut self, evqh: &mut EventLoopHandle, client: &Client,
               resource: &zxdg_popup_v6::ZxdgPopupV6) {
        let ptr = resource.get_user_data();
        let &(ref surface, _, _) = unsafe {
            &*(ptr as
                *mut (
                    wl_surface::WlSurface,
                    zxdg_shell_v6::ZxdgShellV6,
                    zxdg_surface_v6::ZxdgSurfaceV6,
                ))
        };
        self.token
            .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                data.pending_state = ShellSurfacePendingState::None;
                data.configured = false;
            })
            .expect(
                "xdg_toplevel exists but surface has not shell_surface role?!",
            );
        // remove this surface from the known ones (as well as any leftover dead surface)
        self.known_popups.retain(|other| {
            other
                .get_surface()
                .map(|s| !s.equals(surface))
                .unwrap_or(false)
        });
    }

    fn grab(&mut self, evqh: &mut EventLoopHandle, client: &Client, resource: &zxdg_popup_v6::ZxdgPopupV6,
            seat: &wl_seat::WlSeat, serial: u32) {
        let handle = make_popup_handle(self.token, resource);
        self.handler.grab(evqh, handle, seat, serial);
    }
}

server_declare_handler!(
    ShellHandler<U: [Send], R: [Role<ShellSurfaceRole>, Send], H:[CompositorHandler<U, R>, Send], SH: [UserHandler<U,R,H,SD>, Send], SD: [Send]>,
    zxdg_popup_v6::Handler,
    zxdg_popup_v6::ZxdgPopupV6
);
