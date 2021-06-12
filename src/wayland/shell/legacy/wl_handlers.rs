use std::{
    cell::RefCell,
    ops::Deref as _,
    rc::Rc,
    sync::{Arc, Mutex},
};

use wayland_server::{
    protocol::{wl_shell, wl_shell_surface, wl_surface},
    Filter, Main,
};

use crate::wayland::compositor::{roles::Role, CompositorToken};
use crate::wayland::Serial;

use super::{ShellRequest, ShellState, ShellSurface, ShellSurfaceKind, ShellSurfaceRole};

pub(crate) fn implement_shell<R, Impl>(
    shell: Main<wl_shell::WlShell>,
    ctoken: CompositorToken<R>,
    implementation: Rc<RefCell<Impl>>,
    state: Arc<Mutex<ShellState<R>>>,
) where
    R: Role<ShellSurfaceRole> + 'static,
    Impl: FnMut(ShellRequest<R>) + 'static,
{
    shell.quick_assign(move |shell, req, _data| {
        let (id, surface) = match req {
            wl_shell::Request::GetShellSurface { id, surface } => (id, surface),
            _ => unreachable!(),
        };
        let role_data = ShellSurfaceRole {
            title: "".into(),
            class: "".into(),
            pending_ping: None,
        };
        if ctoken.give_role_with(&surface, role_data).is_err() {
            shell
                .as_ref()
                .post_error(wl_shell::Error::Role as u32, "Surface already has a role.".into());
            return;
        }
        let shell_surface =
            implement_shell_surface(id, surface, implementation.clone(), ctoken, state.clone());
        state
            .lock()
            .unwrap()
            .known_surfaces
            .push(make_handle(&shell_surface, ctoken));
        let mut imp = implementation.borrow_mut();
        (&mut *imp)(ShellRequest::NewShellSurface {
            surface: make_handle(&shell_surface, ctoken),
        });
    });
}

fn make_handle<R>(
    shell_surface: &wl_shell_surface::WlShellSurface,
    token: CompositorToken<R>,
) -> ShellSurface<R>
where
    R: Role<ShellSurfaceRole> + 'static,
{
    let data = shell_surface
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData<R>>()
        .unwrap();
    ShellSurface {
        wl_surface: data.surface.clone(),
        shell_surface: shell_surface.clone(),
        token,
    }
}

pub(crate) struct ShellSurfaceUserData<R> {
    surface: wl_surface::WlSurface,
    state: Arc<Mutex<ShellState<R>>>,
}

fn implement_shell_surface<R, Impl>(
    shell_surface: Main<wl_shell_surface::WlShellSurface>,
    surface: wl_surface::WlSurface,
    implementation: Rc<RefCell<Impl>>,
    ctoken: CompositorToken<R>,
    state: Arc<Mutex<ShellState<R>>>,
) -> wl_shell_surface::WlShellSurface
where
    R: Role<ShellSurfaceRole> + 'static,
    Impl: FnMut(ShellRequest<R>) + 'static,
{
    use self::wl_shell_surface::Request;
    shell_surface.quick_assign(move |shell_surface, req, _data| {
        let data = shell_surface
            .as_ref()
            .user_data()
            .get::<ShellSurfaceUserData<R>>()
            .unwrap();
        let mut user_impl = implementation.borrow_mut();
        match req {
            Request::Pong { serial } => {
                let serial = Serial::from(serial);
                let valid = ctoken
                    .with_role_data(&data.surface, |data| {
                        if data.pending_ping == Some(serial) {
                            data.pending_ping = None;
                            true
                        } else {
                            false
                        }
                    })
                    .expect("wl_shell_surface exists but surface has not the right role?");
                if valid {
                    (&mut *user_impl)(ShellRequest::Pong {
                        surface: make_handle(&shell_surface, ctoken),
                    });
                }
            }
            Request::Move { seat, serial } => {
                let serial = Serial::from(serial);
                (&mut *user_impl)(ShellRequest::Move {
                    surface: make_handle(&shell_surface, ctoken),
                    serial,
                    seat,
                })
            }
            Request::Resize { seat, serial, edges } => {
                let serial = Serial::from(serial);
                (&mut *user_impl)(ShellRequest::Resize {
                    surface: make_handle(&shell_surface, ctoken),
                    serial,
                    seat,
                    edges,
                })
            }
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
            } => {
                let serial = Serial::from(serial);
                (&mut *user_impl)(ShellRequest::SetKind {
                    surface: make_handle(&shell_surface, ctoken),
                    kind: ShellSurfaceKind::Popup {
                        parent,
                        serial,
                        seat,
                        location: (x, y),
                        inactive: flags.contains(wl_shell_surface::Transient::Inactive),
                    },
                })
            }
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
            _ => unreachable!(),
        }
    });

    shell_surface.assign_destructor(Filter::new(
        |shell_surface: wl_shell_surface::WlShellSurface, _, _data| {
            let data = shell_surface
                .as_ref()
                .user_data()
                .get::<ShellSurfaceUserData<R>>()
                .unwrap();
            data.state.lock().unwrap().cleanup_surfaces();
        },
    ));

    shell_surface
        .as_ref()
        .user_data()
        .set_threadsafe(|| ShellSurfaceUserData { surface, state });

    shell_surface.deref().clone()
}
