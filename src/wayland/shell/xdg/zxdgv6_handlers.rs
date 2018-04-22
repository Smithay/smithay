use super::{make_shell_client_data, PopupConfigure, PopupKind, PopupState, PositionerState, ShellClient,
            ShellClientData, ShellEvent, ShellImplementation, ShellSurfacePendingState, ShellSurfaceRole,
            ToplevelConfigure, ToplevelKind, ToplevelState};
use std::sync::Mutex;
use utils::Rectangle;
use wayland::compositor::CompositorToken;
use wayland::compositor::roles::*;
use wayland_protocols::xdg_shell::server::{xdg_positioner, xdg_toplevel};
use wayland_protocols::unstable::xdg_shell::v6::server::{zxdg_popup_v6, zxdg_positioner_v6, zxdg_shell_v6,
                                                         zxdg_surface_v6, zxdg_toplevel_v6};
use wayland_server::{LoopToken, NewResource, Resource};
use wayland_server::commons::{downcast_impl, Implementation};
use wayland_server::protocol::wl_surface;

pub(crate) fn implement_shell<U, R, SD>(
    shell: NewResource<zxdg_shell_v6::ZxdgShellV6>,
    implem: &ShellImplementation<U, R, SD>,
) -> Resource<zxdg_shell_v6::ZxdgShellV6>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: Default + 'static,
{
    let shell = shell.implement_nonsend(
        implem.clone(),
        Some(|shell, _| destroy_shell::<SD>(&shell)),
        &implem.loop_token,
    );
    shell.set_user_data(Box::into_raw(Box::new(Mutex::new(make_shell_client_data::<SD>()))) as *mut _);
    let mut user_impl = implem.user_impl.borrow_mut();
    user_impl.receive(
        ShellEvent::NewClient {
            client: make_shell_client(&shell),
        },
        (),
    );
    shell
}

/*
 * xdg_shell
 */

pub(crate) type ShellUserData<SD> = Mutex<ShellClientData<SD>>;

fn destroy_shell<SD>(shell: &Resource<zxdg_shell_v6::ZxdgShellV6>) {
    let ptr = shell.get_user_data();
    shell.set_user_data(::std::ptr::null_mut());
    let data = unsafe { Box::from_raw(ptr as *mut ShellUserData<SD>) };
    // explicit call to drop to not forget what we're doing here
    ::std::mem::drop(data);
}

pub(crate) fn make_shell_client<SD>(resource: &Resource<zxdg_shell_v6::ZxdgShellV6>) -> ShellClient<SD> {
    ShellClient {
        kind: super::ShellClientKind::ZxdgV6(resource.clone()),
        _data: ::std::marker::PhantomData,
    }
}

impl<U, R, SD> Implementation<Resource<zxdg_shell_v6::ZxdgShellV6>, zxdg_shell_v6::Request>
    for ShellImplementation<U, R, SD>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: 'static,
{
    fn receive(&mut self, request: zxdg_shell_v6::Request, shell: Resource<zxdg_shell_v6::ZxdgShellV6>) {
        match request {
            zxdg_shell_v6::Request::Destroy => {
                // all is handled by destructor
            }
            zxdg_shell_v6::Request::CreatePositioner { id } => {
                implement_positioner(id, &self.loop_token);
            }
            zxdg_shell_v6::Request::GetXdgSurface { id, surface } => {
                let role_data = ShellSurfaceRole {
                    pending_state: ShellSurfacePendingState::None,
                    window_geometry: None,
                    pending_configures: Vec::new(),
                    configured: false,
                };
                if self.compositor_token
                    .give_role_with(&surface, role_data)
                    .is_err()
                {
                    shell.post_error(
                        zxdg_shell_v6::Error::Role as u32,
                        "Surface already has a role.".into(),
                    );
                    return;
                }
                let xdg_surface = id.implement_nonsend(
                    self.clone(),
                    Some(destroy_surface::<U, R, SD>),
                    &self.loop_token,
                );
                xdg_surface
                    .set_user_data(Box::into_raw(Box::new((surface.clone(), shell.clone()))) as *mut _);
            }
            zxdg_shell_v6::Request::Pong { serial } => {
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
                    let mut user_impl = self.user_impl.borrow_mut();
                    user_impl.receive(
                        ShellEvent::ClientPong {
                            client: make_shell_client(&shell),
                        },
                        (),
                    );
                }
            }
        }
    }
}

/*
 * xdg_positioner
 */

fn destroy_positioner(positioner: &Resource<zxdg_positioner_v6::ZxdgPositionerV6>) {
    let ptr = positioner.get_user_data();
    positioner.set_user_data(::std::ptr::null_mut());
    // drop the PositionerState
    let surface = unsafe { Box::from_raw(ptr as *mut PositionerState) };
    // explicit call to drop to not forget what we're doing here
    ::std::mem::drop(surface);
}

fn implement_positioner(
    positioner: NewResource<zxdg_positioner_v6::ZxdgPositionerV6>,
    token: &LoopToken,
) -> Resource<zxdg_positioner_v6::ZxdgPositionerV6> {
    let positioner = positioner.implement_nonsend(
        |request, positioner: Resource<_>| {
            let ptr = positioner.get_user_data();
            let state = unsafe { &mut *(ptr as *mut PositionerState) };
            match request {
                zxdg_positioner_v6::Request::Destroy => {
                    // handled by destructor
                }
                zxdg_positioner_v6::Request::SetSize { width, height } => {
                    if width < 1 || height < 1 {
                        positioner.post_error(
                            zxdg_positioner_v6::Error::InvalidInput as u32,
                            "Invalid size for positioner.".into(),
                        );
                    } else {
                        state.rect_size = (width, height);
                    }
                }
                zxdg_positioner_v6::Request::SetAnchorRect {
                    x,
                    y,
                    width,
                    height,
                } => {
                    if width < 1 || height < 1 {
                        positioner.post_error(
                            zxdg_positioner_v6::Error::InvalidInput as u32,
                            "Invalid size for positioner's anchor rectangle.".into(),
                        );
                    } else {
                        state.anchor_rect = Rectangle {
                            x,
                            y,
                            width,
                            height,
                        };
                    }
                }
                zxdg_positioner_v6::Request::SetAnchor { anchor } => {
                    if let Some(anchor) = zxdg_anchor_to_xdg(anchor) {
                        state.anchor_edges = anchor;
                    } else {
                        positioner.post_error(
                            zxdg_positioner_v6::Error::InvalidInput as u32,
                            "Invalid anchor for positioner.".into(),
                        );
                    }
                }
                zxdg_positioner_v6::Request::SetGravity { gravity } => {
                    if let Some(gravity) = zxdg_gravity_to_xdg(gravity) {
                        state.gravity = gravity;
                    } else {
                        positioner.post_error(
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
            }
        },
        Some(|positioner, _| destroy_positioner(&positioner)),
        token,
    );
    let data = PositionerState::new();
    positioner.set_user_data(Box::into_raw(Box::new(data)) as *mut _);
    positioner
}

/*
 * xdg_surface
 */

type XdgSurfaceUserData = (
    Resource<wl_surface::WlSurface>,
    Resource<zxdg_shell_v6::ZxdgShellV6>,
);

fn destroy_surface<U, R, SD>(
    surface: Resource<zxdg_surface_v6::ZxdgSurfaceV6>,
    implem: Box<Implementation<Resource<zxdg_surface_v6::ZxdgSurfaceV6>, zxdg_surface_v6::Request>>,
) where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: 'static,
{
    let implem: ShellImplementation<U, R, SD> = *downcast_impl(implem).unwrap_or_else(|_| unreachable!());
    let ptr = surface.get_user_data();
    surface.set_user_data(::std::ptr::null_mut());
    // take back ownership of the userdata
    let data = unsafe { Box::from_raw(ptr as *mut XdgSurfaceUserData) };
    if !data.0.is_alive() {
        // the wl_surface is destroyed, this means the client is not
        // trying to change the role but it's a cleanup (possibly a
        // disconnecting client), ignore the protocol check.
        return;
    }
    implem
        .compositor_token
        .with_role_data::<ShellSurfaceRole, _, _>(&data.0, |rdata| {
            if let ShellSurfacePendingState::None = rdata.pending_state {
                // all is good
            } else {
                data.1.post_error(
                    zxdg_shell_v6::Error::Role as u32,
                    "xdg_surface was destroyed before its role object".into(),
                );
            }
        })
        .expect("xdg_surface exists but surface has not shell_surface role?!");
}

impl<U, R, SD> Implementation<Resource<zxdg_surface_v6::ZxdgSurfaceV6>, zxdg_surface_v6::Request>
    for ShellImplementation<U, R, SD>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: 'static,
{
    fn receive(
        &mut self,
        request: zxdg_surface_v6::Request,
        xdg_surface: Resource<zxdg_surface_v6::ZxdgSurfaceV6>,
    ) {
        let ptr = xdg_surface.get_user_data();
        let &(ref surface, ref shell) = unsafe { &*(ptr as *mut XdgSurfaceUserData) };
        match request {
            zxdg_surface_v6::Request::Destroy => {
                // all is handled by our destructor
            }
            zxdg_surface_v6::Request::GetToplevel { id } => {
                self.compositor_token
                    .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                        data.pending_state = ShellSurfacePendingState::Toplevel(ToplevelState {
                            parent: None,
                            title: String::new(),
                            app_id: String::new(),
                            min_size: (0, 0),
                            max_size: (0, 0),
                        });
                    })
                    .expect("xdg_surface exists but surface has not shell_surface role?!");
                let toplevel = id.implement_nonsend(
                    self.clone(),
                    Some(destroy_toplevel::<U, R, SD>),
                    &self.loop_token,
                );
                toplevel.set_user_data(Box::into_raw(Box::new((
                    surface.clone(),
                    shell.clone(),
                    xdg_surface.clone(),
                ))) as *mut _);

                self.shell_state
                    .lock()
                    .unwrap()
                    .known_toplevels
                    .push(make_toplevel_handle(self.compositor_token, &toplevel));

                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(ShellEvent::NewToplevel { surface: handle }, ());
            }
            zxdg_surface_v6::Request::GetPopup {
                id,
                parent,
                positioner,
            } => {
                let positioner_data = unsafe { &*(positioner.get_user_data() as *const PositionerState) };

                let parent_ptr = parent.get_user_data();
                let &(ref parent_surface, _) = unsafe { &*(parent_ptr as *mut XdgSurfaceUserData) };
                self.compositor_token
                    .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                        data.pending_state = ShellSurfacePendingState::Popup(PopupState {
                            parent: Some(parent_surface.clone()),
                            positioner: positioner_data.clone(),
                        });
                    })
                    .expect("xdg_surface exists but surface has not shell_surface role?!");
                let popup = id.implement_nonsend(
                    self.clone(),
                    Some(destroy_popup::<U, R, SD>),
                    &self.loop_token,
                );
                popup.set_user_data(Box::into_raw(Box::new((
                    surface.clone(),
                    shell.clone(),
                    xdg_surface.clone(),
                ))) as *mut _);

                self.shell_state
                    .lock()
                    .unwrap()
                    .known_popups
                    .push(make_popup_handle(self.compositor_token, &popup));

                let handle = make_popup_handle(self.compositor_token, &popup);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(ShellEvent::NewPopup { surface: handle }, ());
            }
            zxdg_surface_v6::Request::SetWindowGeometry {
                x,
                y,
                width,
                height,
            } => {
                self.compositor_token
                    .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                        data.window_geometry = Some(Rectangle {
                            x,
                            y,
                            width,
                            height,
                        });
                    })
                    .expect("xdg_surface exists but surface has not shell_surface role?!");
            }
            zxdg_surface_v6::Request::AckConfigure { serial } => {
                self.compositor_token
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
                    .expect("xdg_surface exists but surface has not shell_surface role?!");
            }
        }
    }
}

/*
 * xdg_toplevel
 */

pub type ShellSurfaceUserData = (
    Resource<wl_surface::WlSurface>,
    Resource<zxdg_shell_v6::ZxdgShellV6>,
    Resource<zxdg_surface_v6::ZxdgSurfaceV6>,
);

// Utility functions allowing to factor out a lot of the upcoming logic
fn with_surface_toplevel_data<U, R, SD, F>(
    implem: &ShellImplementation<U, R, SD>,
    toplevel: &Resource<zxdg_toplevel_v6::ZxdgToplevelV6>,
    f: F,
) where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: 'static,
    F: FnOnce(&mut ToplevelState),
{
    let ptr = toplevel.get_user_data();
    let &(ref surface, _, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
    implem
        .compositor_token
        .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| match data.pending_state {
            ShellSurfacePendingState::Toplevel(ref mut toplevel_data) => f(toplevel_data),
            _ => unreachable!(),
        })
        .expect("xdg_toplevel exists but surface has not shell_surface role?!");
}

pub fn send_toplevel_configure<U, R>(
    token: CompositorToken<U, R>,
    resource: &Resource<zxdg_toplevel_v6::ZxdgToplevelV6>,
    configure: ToplevelConfigure,
) where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
{
    let &(ref surface, _, ref shell_surface) =
        unsafe { &*(resource.get_user_data() as *mut ShellSurfaceUserData) };
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
    resource.send(zxdg_toplevel_v6::Event::Configure {
        width,
        height,
        states,
    });
    shell_surface.send(zxdg_surface_v6::Event::Configure { serial });
    // Add the configure as pending
    token
        .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| data.pending_configures.push(serial))
        .expect("xdg_toplevel exists but surface has not shell_surface role?!");
}

fn make_toplevel_handle<U, R, SD>(
    token: CompositorToken<U, R>,
    resource: &Resource<zxdg_toplevel_v6::ZxdgToplevelV6>,
) -> super::ToplevelSurface<U, R, SD> {
    let ptr = resource.get_user_data();
    let &(ref wl_surface, _, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
    super::ToplevelSurface {
        wl_surface: wl_surface.clone(),
        shell_surface: ToplevelKind::ZxdgV6(resource.clone()),
        token: token,
        _shell_data: ::std::marker::PhantomData,
    }
}

impl<U, R, SD> Implementation<Resource<zxdg_toplevel_v6::ZxdgToplevelV6>, zxdg_toplevel_v6::Request>
    for ShellImplementation<U, R, SD>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: 'static,
{
    fn receive(
        &mut self,
        request: zxdg_toplevel_v6::Request,
        toplevel: Resource<zxdg_toplevel_v6::ZxdgToplevelV6>,
    ) {
        match request {
            zxdg_toplevel_v6::Request::Destroy => {
                // all it done by the destructor
            }
            zxdg_toplevel_v6::Request::SetParent { parent } => {
                with_surface_toplevel_data(self, &toplevel, |toplevel_data| {
                    toplevel_data.parent = parent.map(|toplevel_surface_parent| {
                        let parent_ptr = toplevel_surface_parent.get_user_data();
                        let &(ref parent_surface, _, _) =
                            unsafe { &*(parent_ptr as *mut ShellSurfaceUserData) };
                        parent_surface.clone()
                    })
                });
            }
            zxdg_toplevel_v6::Request::SetTitle { title } => {
                with_surface_toplevel_data(self, &toplevel, |toplevel_data| {
                    toplevel_data.title = title;
                });
            }
            zxdg_toplevel_v6::Request::SetAppId { app_id } => {
                with_surface_toplevel_data(self, &toplevel, |toplevel_data| {
                    toplevel_data.app_id = app_id;
                });
            }
            zxdg_toplevel_v6::Request::ShowWindowMenu { seat, serial, x, y } => {
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(
                    ShellEvent::ShowWindowMenu {
                        surface: handle,
                        seat,
                        serial,
                        location: (x, y),
                    },
                    (),
                );
            }
            zxdg_toplevel_v6::Request::Move { seat, serial } => {
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(
                    ShellEvent::Move {
                        surface: handle,
                        seat,
                        serial,
                    },
                    (),
                );
            }
            zxdg_toplevel_v6::Request::Resize {
                seat,
                serial,
                edges,
            } => {
                let edges = zxdg_toplevel_v6::ResizeEdge::from_raw(edges)
                    .unwrap_or(zxdg_toplevel_v6::ResizeEdge::None);
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(
                    ShellEvent::Resize {
                        surface: handle,
                        seat,
                        serial,
                        edges: zxdg_edges_to_xdg(edges),
                    },
                    (),
                );
            }
            zxdg_toplevel_v6::Request::SetMaxSize { width, height } => {
                with_surface_toplevel_data(self, &toplevel, |toplevel_data| {
                    toplevel_data.max_size = (width, height);
                });
            }
            zxdg_toplevel_v6::Request::SetMinSize { width, height } => {
                with_surface_toplevel_data(self, &toplevel, |toplevel_data| {
                    toplevel_data.max_size = (width, height);
                });
            }
            zxdg_toplevel_v6::Request::SetMaximized => {
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(ShellEvent::Maximize { surface: handle }, ());
            }
            zxdg_toplevel_v6::Request::UnsetMaximized => {
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(ShellEvent::UnMaximize { surface: handle }, ());
            }
            zxdg_toplevel_v6::Request::SetFullscreen { output } => {
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(
                    ShellEvent::Fullscreen {
                        surface: handle,
                        output,
                    },
                    (),
                );
            }
            zxdg_toplevel_v6::Request::UnsetFullscreen => {
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(ShellEvent::UnFullscreen { surface: handle }, ());
            }
            zxdg_toplevel_v6::Request::SetMinimized => {
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(ShellEvent::Minimize { surface: handle }, ());
            }
        }
    }
}

fn destroy_toplevel<U, R, SD>(
    toplevel: Resource<zxdg_toplevel_v6::ZxdgToplevelV6>,
    implem: Box<Implementation<Resource<zxdg_toplevel_v6::ZxdgToplevelV6>, zxdg_toplevel_v6::Request>>,
) where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: 'static,
{
    let implem: ShellImplementation<U, R, SD> = *downcast_impl(implem).unwrap_or_else(|_| unreachable!());
    let ptr = toplevel.get_user_data();
    toplevel.set_user_data(::std::ptr::null_mut());
    // take back ownership of the userdata
    let data = *unsafe { Box::from_raw(ptr as *mut ShellSurfaceUserData) };
    if !data.0.is_alive() {
        // the wl_surface is destroyed, this means the client is not
        // trying to change the role but it's a cleanup (possibly a
        // disconnecting client), ignore the protocol check.
        return;
    } else {
        implem
            .compositor_token
            .with_role_data::<ShellSurfaceRole, _, _>(&data.0, |data| {
                data.pending_state = ShellSurfacePendingState::None;
                data.configured = false;
            })
            .expect("xdg_toplevel exists but surface has not shell_surface role?!");
    }
    // remove this surface from the known ones (as well as any leftover dead surface)
    implem
        .shell_state
        .lock()
        .unwrap()
        .known_toplevels
        .retain(|other| other.alive());
}

/*
 * xdg_popup
 */

pub(crate) fn send_popup_configure<U, R>(
    token: CompositorToken<U, R>,
    resource: &Resource<zxdg_popup_v6::ZxdgPopupV6>,
    configure: PopupConfigure,
) where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
{
    let &(ref surface, _, ref shell_surface) =
        unsafe { &*(resource.get_user_data() as *mut ShellSurfaceUserData) };
    let (x, y) = configure.position;
    let (width, height) = configure.size;
    let serial = configure.serial;
    resource.send(zxdg_popup_v6::Event::Configure {
        x,
        y,
        width,
        height,
    });
    shell_surface.send(zxdg_surface_v6::Event::Configure { serial });
    // Add the configure as pending
    token
        .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| data.pending_configures.push(serial))
        .expect("xdg_toplevel exists but surface has not shell_surface role?!");
}

fn make_popup_handle<U, R, SD>(
    token: CompositorToken<U, R>,
    resource: &Resource<zxdg_popup_v6::ZxdgPopupV6>,
) -> super::PopupSurface<U, R, SD> {
    let ptr = resource.get_user_data();
    let &(ref wl_surface, _, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
    super::PopupSurface {
        wl_surface: wl_surface.clone(),
        shell_surface: PopupKind::ZxdgV6(resource.clone()),
        token: token,
        _shell_data: ::std::marker::PhantomData,
    }
}

impl<U, R, SD> Implementation<Resource<zxdg_popup_v6::ZxdgPopupV6>, zxdg_popup_v6::Request>
    for ShellImplementation<U, R, SD>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: 'static,
{
    fn receive(&mut self, request: zxdg_popup_v6::Request, popup: Resource<zxdg_popup_v6::ZxdgPopupV6>) {
        match request {
            zxdg_popup_v6::Request::Destroy => {
                // all is handled by our destructor
            }
            zxdg_popup_v6::Request::Grab { seat, serial } => {
                let handle = make_popup_handle(self.compositor_token, &popup);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(
                    ShellEvent::Grab {
                        surface: handle,
                        seat,
                        serial,
                    },
                    (),
                );
            }
        }
    }
}

fn destroy_popup<U, R, SD>(
    popup: Resource<zxdg_popup_v6::ZxdgPopupV6>,
    implem: Box<Implementation<Resource<zxdg_popup_v6::ZxdgPopupV6>, zxdg_popup_v6::Request>>,
) where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: 'static,
{
    let implem: ShellImplementation<U, R, SD> = *downcast_impl(implem).unwrap_or_else(|_| unreachable!());
    let ptr = popup.get_user_data();
    popup.set_user_data(::std::ptr::null_mut());
    // take back ownership of the userdata
    let data = *unsafe { Box::from_raw(ptr as *mut ShellSurfaceUserData) };
    if !data.0.is_alive() {
        // the wl_surface is destroyed, this means the client is not
        // trying to change the role but it's a cleanup (possibly a
        // disconnecting client), ignore the protocol check.
        return;
    } else {
        implem
            .compositor_token
            .with_role_data::<ShellSurfaceRole, _, _>(&data.0, |data| {
                data.pending_state = ShellSurfacePendingState::None;
                data.configured = false;
            })
            .expect("xdg_popup exists but surface has not shell_surface role?!");
    }
    // remove this surface from the known ones (as well as any leftover dead surface)
    implem
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
