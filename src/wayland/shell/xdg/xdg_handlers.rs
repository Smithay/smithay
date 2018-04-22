use super::{make_shell_client_data, PopupConfigure, PopupKind, PopupState, PositionerState, ShellClient,
            ShellClientData, ShellEvent, ShellImplementation, ShellSurfacePendingState, ShellSurfaceRole,
            ToplevelConfigure, ToplevelKind, ToplevelState};
use std::sync::Mutex;
use utils::Rectangle;
use wayland::compositor::CompositorToken;
use wayland::compositor::roles::*;
use wayland_protocols::xdg_shell::server::{xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_server::{LoopToken, NewResource, Resource};
use wayland_server::commons::{downcast_impl, Implementation};
use wayland_server::protocol::wl_surface;

pub(crate) fn implement_wm_base<U, R, SD>(
    shell: NewResource<xdg_wm_base::XdgWmBase>,
    implem: &ShellImplementation<U, R, SD>,
) -> Resource<xdg_wm_base::XdgWmBase>
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

fn destroy_shell<SD>(shell: &Resource<xdg_wm_base::XdgWmBase>) {
    let ptr = shell.get_user_data();
    shell.set_user_data(::std::ptr::null_mut());
    let data = unsafe { Box::from_raw(ptr as *mut ShellUserData<SD>) };
    // explicit call to drop to not forget what we're doing here
    ::std::mem::drop(data);
}

pub(crate) fn make_shell_client<SD>(resource: &Resource<xdg_wm_base::XdgWmBase>) -> ShellClient<SD> {
    ShellClient {
        kind: super::ShellClientKind::Xdg(resource.clone()),
        _data: ::std::marker::PhantomData,
    }
}

impl<U, R, SD> Implementation<Resource<xdg_wm_base::XdgWmBase>, xdg_wm_base::Request>
    for ShellImplementation<U, R, SD>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: 'static,
{
    fn receive(&mut self, request: xdg_wm_base::Request, shell: Resource<xdg_wm_base::XdgWmBase>) {
        match request {
            xdg_wm_base::Request::Destroy => {
                // all is handled by destructor
            }
            xdg_wm_base::Request::CreatePositioner { id } => {
                implement_positioner(id, &self.loop_token);
            }
            xdg_wm_base::Request::GetXdgSurface { id, surface } => {
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
                        xdg_wm_base::Error::Role as u32,
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
            xdg_wm_base::Request::Pong { serial } => {
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

fn destroy_positioner(positioner: &Resource<xdg_positioner::XdgPositioner>) {
    let ptr = positioner.get_user_data();
    positioner.set_user_data(::std::ptr::null_mut());
    // drop the PositionerState
    let surface = unsafe { Box::from_raw(ptr as *mut PositionerState) };
    // explicit call to drop to not forget what we're doing here
    ::std::mem::drop(surface);
}

fn implement_positioner(
    positioner: NewResource<xdg_positioner::XdgPositioner>,
    token: &LoopToken,
) -> Resource<xdg_positioner::XdgPositioner> {
    let positioner = positioner.implement_nonsend(
        |request, positioner: Resource<_>| {
            let ptr = positioner.get_user_data();
            let state = unsafe { &mut *(ptr as *mut PositionerState) };
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
                xdg_positioner::Request::SetAnchorRect {
                    x,
                    y,
                    width,
                    height,
                } => {
                    if width < 1 || height < 1 {
                        positioner.post_error(
                            xdg_positioner::Error::InvalidInput as u32,
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
    Resource<xdg_wm_base::XdgWmBase>,
);

fn destroy_surface<U, R, SD>(
    surface: Resource<xdg_surface::XdgSurface>,
    implem: Box<Implementation<Resource<xdg_surface::XdgSurface>, xdg_surface::Request>>,
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
                    xdg_wm_base::Error::Role as u32,
                    "xdg_surface was destroyed before its role object".into(),
                );
            }
        })
        .expect("xdg_surface exists but surface has not shell_surface role?!");
}

impl<U, R, SD> Implementation<Resource<xdg_surface::XdgSurface>, xdg_surface::Request>
    for ShellImplementation<U, R, SD>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: 'static,
{
    fn receive(&mut self, request: xdg_surface::Request, xdg_surface: Resource<xdg_surface::XdgSurface>) {
        let ptr = xdg_surface.get_user_data();
        let &(ref surface, ref shell) = unsafe { &*(ptr as *mut XdgSurfaceUserData) };
        match request {
            xdg_surface::Request::Destroy => {
                // all is handled by our destructor
            }
            xdg_surface::Request::GetToplevel { id } => {
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
            xdg_surface::Request::GetPopup {
                id,
                parent,
                positioner,
            } => {
                let positioner_data = unsafe { &*(positioner.get_user_data() as *const PositionerState) };

                let parent_surface = parent.map(|parent| {
                    let parent_ptr = parent.get_user_data();
                    let &(ref parent_surface, _) = unsafe { &*(parent_ptr as *mut XdgSurfaceUserData) };
                    parent_surface.clone()
                });
                self.compositor_token
                    .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| {
                        data.pending_state = ShellSurfacePendingState::Popup(PopupState {
                            parent: parent_surface,
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
            xdg_surface::Request::SetWindowGeometry {
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
            xdg_surface::Request::AckConfigure { serial } => {
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
                                xdg_wm_base::Error::InvalidSurfaceState as u32,
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
    Resource<xdg_wm_base::XdgWmBase>,
    Resource<xdg_surface::XdgSurface>,
);

// Utility functions allowing to factor out a lot of the upcoming logic
fn with_surface_toplevel_data<U, R, SD, F>(
    implem: &ShellImplementation<U, R, SD>,
    toplevel: &Resource<xdg_toplevel::XdgToplevel>,
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
    resource: &Resource<xdg_toplevel::XdgToplevel>,
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
    resource.send(xdg_toplevel::Event::Configure {
        width,
        height,
        states,
    });
    shell_surface.send(xdg_surface::Event::Configure { serial });
    // Add the configure as pending
    token
        .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| data.pending_configures.push(serial))
        .expect("xdg_toplevel exists but surface has not shell_surface role?!");
}

fn make_toplevel_handle<U, R, SD>(
    token: CompositorToken<U, R>,
    resource: &Resource<xdg_toplevel::XdgToplevel>,
) -> super::ToplevelSurface<U, R, SD> {
    let ptr = resource.get_user_data();
    let &(ref wl_surface, _, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
    super::ToplevelSurface {
        wl_surface: wl_surface.clone(),
        shell_surface: ToplevelKind::Xdg(resource.clone()),
        token: token,
        _shell_data: ::std::marker::PhantomData,
    }
}

impl<U, R, SD> Implementation<Resource<xdg_toplevel::XdgToplevel>, xdg_toplevel::Request>
    for ShellImplementation<U, R, SD>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: 'static,
{
    fn receive(&mut self, request: xdg_toplevel::Request, toplevel: Resource<xdg_toplevel::XdgToplevel>) {
        match request {
            xdg_toplevel::Request::Destroy => {
                // all it done by the destructor
            }
            xdg_toplevel::Request::SetParent { parent } => {
                with_surface_toplevel_data(self, &toplevel, |toplevel_data| {
                    toplevel_data.parent = parent.map(|toplevel_surface_parent| {
                        let parent_ptr = toplevel_surface_parent.get_user_data();
                        let &(ref parent_surface, _, _) =
                            unsafe { &*(parent_ptr as *mut ShellSurfaceUserData) };
                        parent_surface.clone()
                    })
                });
            }
            xdg_toplevel::Request::SetTitle { title } => {
                with_surface_toplevel_data(self, &toplevel, |toplevel_data| {
                    toplevel_data.title = title;
                });
            }
            xdg_toplevel::Request::SetAppId { app_id } => {
                with_surface_toplevel_data(self, &toplevel, |toplevel_data| {
                    toplevel_data.app_id = app_id;
                });
            }
            xdg_toplevel::Request::ShowWindowMenu { seat, serial, x, y } => {
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
            xdg_toplevel::Request::Move { seat, serial } => {
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
            xdg_toplevel::Request::Resize {
                seat,
                serial,
                edges,
            } => {
                let edges =
                    xdg_toplevel::ResizeEdge::from_raw(edges).unwrap_or(xdg_toplevel::ResizeEdge::None);
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(
                    ShellEvent::Resize {
                        surface: handle,
                        seat,
                        serial,
                        edges,
                    },
                    (),
                );
            }
            xdg_toplevel::Request::SetMaxSize { width, height } => {
                with_surface_toplevel_data(self, &toplevel, |toplevel_data| {
                    toplevel_data.max_size = (width, height);
                });
            }
            xdg_toplevel::Request::SetMinSize { width, height } => {
                with_surface_toplevel_data(self, &toplevel, |toplevel_data| {
                    toplevel_data.max_size = (width, height);
                });
            }
            xdg_toplevel::Request::SetMaximized => {
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(ShellEvent::Maximize { surface: handle }, ());
            }
            xdg_toplevel::Request::UnsetMaximized => {
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(ShellEvent::UnMaximize { surface: handle }, ());
            }
            xdg_toplevel::Request::SetFullscreen { output } => {
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
            xdg_toplevel::Request::UnsetFullscreen => {
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(ShellEvent::UnFullscreen { surface: handle }, ());
            }
            xdg_toplevel::Request::SetMinimized => {
                let handle = make_toplevel_handle(self.compositor_token, &toplevel);
                let mut user_impl = self.user_impl.borrow_mut();
                user_impl.receive(ShellEvent::Minimize { surface: handle }, ());
            }
        }
    }
}

fn destroy_toplevel<U, R, SD>(
    toplevel: Resource<xdg_toplevel::XdgToplevel>,
    implem: Box<Implementation<Resource<xdg_toplevel::XdgToplevel>, xdg_toplevel::Request>>,
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
    resource: &Resource<xdg_popup::XdgPopup>,
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
    resource.send(xdg_popup::Event::Configure {
        x,
        y,
        width,
        height,
    });
    shell_surface.send(xdg_surface::Event::Configure { serial });
    // Add the configure as pending
    token
        .with_role_data::<ShellSurfaceRole, _, _>(surface, |data| data.pending_configures.push(serial))
        .expect("xdg_toplevel exists but surface has not shell_surface role?!");
}

fn make_popup_handle<U, R, SD>(
    token: CompositorToken<U, R>,
    resource: &Resource<xdg_popup::XdgPopup>,
) -> super::PopupSurface<U, R, SD> {
    let ptr = resource.get_user_data();
    let &(ref wl_surface, _, _) = unsafe { &*(ptr as *mut ShellSurfaceUserData) };
    super::PopupSurface {
        wl_surface: wl_surface.clone(),
        shell_surface: PopupKind::Xdg(resource.clone()),
        token: token,
        _shell_data: ::std::marker::PhantomData,
    }
}

impl<U, R, SD> Implementation<Resource<xdg_popup::XdgPopup>, xdg_popup::Request>
    for ShellImplementation<U, R, SD>
where
    U: 'static,
    R: Role<ShellSurfaceRole> + 'static,
    SD: 'static,
{
    fn receive(&mut self, request: xdg_popup::Request, popup: Resource<xdg_popup::XdgPopup>) {
        match request {
            xdg_popup::Request::Destroy => {
                // all is handled by our destructor
            }
            xdg_popup::Request::Grab { seat, serial } => {
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
    popup: Resource<xdg_popup::XdgPopup>,
    implem: Box<Implementation<Resource<xdg_popup::XdgPopup>, xdg_popup::Request>>,
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
