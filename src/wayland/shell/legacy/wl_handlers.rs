use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use wayland_server::{LoopToken, NewResource, Resource};
use wayland_server::commons::Implementation;
use wayland_server::protocol::{wl_shell, wl_shell_surface, wl_surface};

use wayland::compositor::CompositorToken;
use wayland::compositor::roles::Role;

use super::{ShellRequest, ShellState, ShellSurface, ShellSurfaceKind, ShellSurfaceRole};

pub(crate) fn implement_shell<U, R, D, Impl>(
    shell: NewResource<wl_shell::WlShell>,
    ltoken: LoopToken,
    ctoken: CompositorToken<U, R>,
    implementation: Rc<RefCell<Impl>>,
    state: Arc<Mutex<ShellState<U, R, D>>>,
) where
    U: 'static,
    D: Default + 'static,
    R: Role<ShellSurfaceRole<D>> + 'static,
    Impl: Implementation<(), ShellRequest<U, R, D>> + 'static,
{
    let ltoken2 = ltoken.clone();
    shell.implement_nonsend(
        move |req, shell: Resource<_>| {
            let wl_shell::Request::GetShellSurface { id, surface } = req;
            let role_data = ShellSurfaceRole {
                title: "".into(),
                class: "".into(),
                pending_ping: 0,
                user_data: Default::default(),
            };
            if ctoken.give_role_with(&surface, role_data).is_err() {
                shell.post_error(
                    wl_shell::Error::Role as u32,
                    "Surface already has a role.".into(),
                );
                return;
            }
            let shell_surface = implement_shell_surface(
                id,
                surface,
                implementation.clone(),
                ltoken.clone(),
                ctoken,
                state.clone(),
            );
            state
                .lock()
                .unwrap()
                .known_surfaces
                .push(make_handle(&shell_surface, ctoken));
            implementation.borrow_mut().receive(
                ShellRequest::NewShellSurface {
                    surface: make_handle(&shell_surface, ctoken),
                },
                (),
            );
        },
        None::<fn(_, _)>,
        &ltoken2,
    );
}

fn make_handle<U, R, D>(
    shell_surface: &Resource<wl_shell_surface::WlShellSurface>,
    token: CompositorToken<U, R>,
) -> ShellSurface<U, R, D> {
    let data = unsafe { &*(shell_surface.get_user_data() as *mut ShellSurfaceUserData<U, R, D>) };
    ShellSurface {
        wl_surface: data.surface.clone(),
        shell_surface: shell_surface.clone(),
        token,
        _d: ::std::marker::PhantomData,
    }
}

pub(crate) struct ShellSurfaceUserData<U, R, D> {
    surface: Resource<wl_surface::WlSurface>,
    state: Arc<Mutex<ShellState<U, R, D>>>,
}

fn implement_shell_surface<U, R, Impl, D>(
    shell_surface: NewResource<wl_shell_surface::WlShellSurface>,
    surface: Resource<wl_surface::WlSurface>,
    implementation: Rc<RefCell<Impl>>,
    ltoken: LoopToken,
    ctoken: CompositorToken<U, R>,
    state: Arc<Mutex<ShellState<U, R, D>>>,
) -> Resource<wl_shell_surface::WlShellSurface>
where
    U: 'static,
    D: 'static,
    R: Role<ShellSurfaceRole<D>> + 'static,
    Impl: Implementation<(), ShellRequest<U, R, D>> + 'static,
{
    use self::wl_shell_surface::Request;
    let shell_surface = shell_surface.implement_nonsend(
        move |req, shell_surface: Resource<_>| {
            let data = unsafe { &mut *(shell_surface.get_user_data() as *mut ShellSurfaceUserData<U, R, D>) };
            let mut user_impl = implementation.borrow_mut();
            match req {
                Request::Pong { serial } => {
                    let valid = ctoken
                        .with_role_data(&data.surface, |data| {
                            if data.pending_ping == serial {
                                data.pending_ping = 0;
                                true
                            } else {
                                false
                            }
                        })
                        .expect("wl_shell_surface exists but surface has not the right role?");
                    if valid {
                        user_impl.receive(
                            ShellRequest::Pong {
                                surface: make_handle(&shell_surface, ctoken),
                            },
                            (),
                        );
                    }
                }
                Request::Move { seat, serial } => user_impl.receive(
                    ShellRequest::Move {
                        surface: make_handle(&shell_surface, ctoken),
                        serial,
                        seat,
                    },
                    (),
                ),
                Request::Resize {
                    seat,
                    serial,
                    edges,
                } => user_impl.receive(
                    ShellRequest::Resize {
                        surface: make_handle(&shell_surface, ctoken),
                        serial,
                        seat,
                        edges,
                    },
                    (),
                ),
                Request::SetToplevel => user_impl.receive(
                    ShellRequest::SetKind {
                        surface: make_handle(&shell_surface, ctoken),
                        kind: ShellSurfaceKind::Toplevel,
                    },
                    (),
                ),
                Request::SetTransient {
                    parent,
                    x,
                    y,
                    flags,
                } => user_impl.receive(
                    ShellRequest::SetKind {
                        surface: make_handle(&shell_surface, ctoken),
                        kind: ShellSurfaceKind::Transient {
                            parent,
                            location: (x, y),
                            inactive: flags.contains(wl_shell_surface::Transient::Inactive),
                        },
                    },
                    (),
                ),
                Request::SetFullscreen {
                    method,
                    framerate,
                    output,
                } => user_impl.receive(
                    ShellRequest::SetKind {
                        surface: make_handle(&shell_surface, ctoken),
                        kind: ShellSurfaceKind::Fullscreen {
                            method,
                            framerate,
                            output,
                        },
                    },
                    (),
                ),
                Request::SetPopup {
                    seat,
                    serial,
                    parent,
                    x,
                    y,
                    flags,
                } => user_impl.receive(
                    ShellRequest::SetKind {
                        surface: make_handle(&shell_surface, ctoken),
                        kind: ShellSurfaceKind::Popup {
                            parent,
                            serial,
                            seat,
                            location: (x, y),
                            inactive: flags.contains(wl_shell_surface::Transient::Inactive),
                        },
                    },
                    (),
                ),
                Request::SetMaximized { output } => user_impl.receive(
                    ShellRequest::SetKind {
                        surface: make_handle(&shell_surface, ctoken),
                        kind: ShellSurfaceKind::Maximized { output },
                    },
                    (),
                ),
                Request::SetTitle { title } => {
                    ctoken
                        .with_role_data(&data.surface, |data| data.title = title)
                        .expect("wl_shell_surface exists but surface has not shell_surface role?!");
                }
                Request::SetClass { class_ } => {
                    ctoken
                        .with_role_data(&data.surface, |data| data.class = class_)
                        .expect("wl_shell_surface exists but surface has not shell_surface role?!");
                }
            }
        },
        Some(|shell_surface: Resource<_>, _| {
            let data =
                unsafe { Box::from_raw(shell_surface.get_user_data() as *mut ShellSurfaceUserData<U, R, D>) };
            data.state.lock().unwrap().cleanup_surfaces();
        }),
        &ltoken,
    );
    shell_surface.set_user_data(Box::into_raw(Box::new(ShellSurfaceUserData { surface, state })) as *mut ());
    shell_surface
}
