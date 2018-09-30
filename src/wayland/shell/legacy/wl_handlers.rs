use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use wayland_server::protocol::{wl_shell, wl_shell_surface, wl_surface};
use wayland_server::{DisplayToken, NewResource, Resource};

use wayland::compositor::roles::Role;
use wayland::compositor::CompositorToken;

use super::{ShellRequest, ShellState, ShellSurface, ShellSurfaceKind, ShellSurfaceRole};

pub(crate) fn implement_shell<U, R, D, Impl>(
    shell: NewResource<wl_shell::WlShell>,
    dtoken: DisplayToken,
    ctoken: CompositorToken<U, R>,
    implementation: Rc<RefCell<Impl>>,
    state: Arc<Mutex<ShellState<U, R, D>>>,
) where
    U: 'static,
    D: Default + 'static,
    R: Role<ShellSurfaceRole<D>> + 'static,
    Impl: FnMut(ShellRequest<U, R, D>) + 'static,
{
    let dtoken2 = dtoken.clone();
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
                shell.post_error(wl_shell::Error::Role as u32, "Surface already has a role.".into());
                return;
            }
            let shell_surface = implement_shell_surface(
                id,
                surface,
                implementation.clone(),
                dtoken.clone(),
                ctoken,
                state.clone(),
            );
            state
                .lock()
                .unwrap()
                .known_surfaces
                .push(make_handle(&shell_surface, ctoken));
            let mut imp = implementation.borrow_mut();
            (&mut *imp)(ShellRequest::NewShellSurface {
                surface: make_handle(&shell_surface, ctoken),
            });
        },
        None::<fn(_)>,
        (),
        &dtoken2,
    );
}

fn make_handle<U, R, SD>(
    shell_surface: &Resource<wl_shell_surface::WlShellSurface>,
    token: CompositorToken<U, R>,
) -> ShellSurface<U, R, SD>
where
    U: 'static,
    R: Role<ShellSurfaceRole<SD>> + 'static,
    SD: 'static,
{
    let data = shell_surface
        .user_data::<ShellSurfaceUserData<U, R, SD>>()
        .unwrap();
    ShellSurface {
        wl_surface: data.surface.clone(),
        shell_surface: shell_surface.clone(),
        token,
        _d: ::std::marker::PhantomData,
    }
}

pub(crate) struct ShellSurfaceUserData<U, R, SD> {
    surface: Resource<wl_surface::WlSurface>,
    state: Arc<Mutex<ShellState<U, R, SD>>>,
}

fn implement_shell_surface<U, R, Impl, SD>(
    shell_surface: NewResource<wl_shell_surface::WlShellSurface>,
    surface: Resource<wl_surface::WlSurface>,
    implementation: Rc<RefCell<Impl>>,
    dtoken: DisplayToken,
    ctoken: CompositorToken<U, R>,
    state: Arc<Mutex<ShellState<U, R, SD>>>,
) -> Resource<wl_shell_surface::WlShellSurface>
where
    U: 'static,
    SD: 'static,
    R: Role<ShellSurfaceRole<SD>> + 'static,
    Impl: FnMut(ShellRequest<U, R, SD>) + 'static,
{
    use self::wl_shell_surface::Request;
    shell_surface.implement_nonsend(
        move |req, shell_surface: Resource<_>| {
            let data = shell_surface
                .user_data::<ShellSurfaceUserData<U, R, SD>>()
                .unwrap();
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
                        }).expect("wl_shell_surface exists but surface has not the right role?");
                    if valid {
                        (&mut *user_impl)(ShellRequest::Pong {
                            surface: make_handle(&shell_surface, ctoken),
                        });
                    }
                }
                Request::Move { seat, serial } => (&mut *user_impl)(ShellRequest::Move {
                    surface: make_handle(&shell_surface, ctoken),
                    serial,
                    seat,
                }),
                Request::Resize { seat, serial, edges } => (&mut *user_impl)(ShellRequest::Resize {
                    surface: make_handle(&shell_surface, ctoken),
                    serial,
                    seat,
                    edges,
                }),
                Request::SetToplevel => (&mut *user_impl)(ShellRequest::SetKind {
                    surface: make_handle(&shell_surface, ctoken),
                    kind: ShellSurfaceKind::Toplevel,
                }),
                Request::SetTransient { parent, x, y, flags } => (&mut *user_impl)(ShellRequest::SetKind {
                    surface: make_handle(&shell_surface, ctoken),
                    kind: ShellSurfaceKind::Transient {
                        parent,
                        location: (x, y),
                        inactive: flags.contains(wl_shell_surface::Transient::Inactive),
                    },
                }),
                Request::SetFullscreen {
                    method,
                    framerate,
                    output,
                } => (&mut *user_impl)(ShellRequest::SetKind {
                    surface: make_handle(&shell_surface, ctoken),
                    kind: ShellSurfaceKind::Fullscreen {
                        method,
                        framerate,
                        output,
                    },
                }),
                Request::SetPopup {
                    seat,
                    serial,
                    parent,
                    x,
                    y,
                    flags,
                } => (&mut *user_impl)(ShellRequest::SetKind {
                    surface: make_handle(&shell_surface, ctoken),
                    kind: ShellSurfaceKind::Popup {
                        parent,
                        serial,
                        seat,
                        location: (x, y),
                        inactive: flags.contains(wl_shell_surface::Transient::Inactive),
                    },
                }),
                Request::SetMaximized { output } => (&mut *user_impl)(ShellRequest::SetKind {
                    surface: make_handle(&shell_surface, ctoken),
                    kind: ShellSurfaceKind::Maximized { output },
                }),
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
        Some(|shell_surface: Resource<_>| {
            let data = shell_surface
                .user_data::<ShellSurfaceUserData<U, R, SD>>()
                .unwrap();
            data.state.lock().unwrap().cleanup_surfaces();
        }),
        ShellSurfaceUserData { surface, state },
        &dtoken,
    )
}
