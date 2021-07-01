use std::{
    cell::RefCell,
    ops::Deref as _,
    rc::Rc,
    sync::{Arc, Mutex},
};

use wayland_server::{
    protocol::{wl_shell, wl_shell_surface, wl_surface},
    DispatchData, Filter, Main,
};

static WL_SHELL_SURFACE_ROLE: &str = "wl_shell_surface";

use crate::wayland::compositor;
use crate::wayland::Serial;

use super::{ShellRequest, ShellState, ShellSurface, ShellSurfaceAttributes, ShellSurfaceKind};

pub(crate) fn implement_shell<Impl>(
    shell: Main<wl_shell::WlShell>,
    implementation: Rc<RefCell<Impl>>,
    state: Arc<Mutex<ShellState>>,
) where
    Impl: FnMut(ShellRequest, DispatchData<'_>) + 'static,
{
    shell.quick_assign(move |shell, req, data| {
        let (id, surface) = match req {
            wl_shell::Request::GetShellSurface { id, surface } => (id, surface),
            _ => unreachable!(),
        };
        if compositor::give_role(&surface, WL_SHELL_SURFACE_ROLE).is_err() {
            shell
                .as_ref()
                .post_error(wl_shell::Error::Role as u32, "Surface already has a role.".into());
            return;
        }
        compositor::with_states(&surface, |states| {
            states.data_map.insert_if_missing(|| {
                Mutex::new(ShellSurfaceAttributes {
                    title: "".into(),
                    class: "".into(),
                    pending_ping: None,
                })
            })
        })
        .unwrap();
        let shell_surface = implement_shell_surface(id, surface, implementation.clone(), state.clone());
        state
            .lock()
            .unwrap()
            .known_surfaces
            .push(make_handle(&shell_surface));
        let mut imp = implementation.borrow_mut();
        (&mut *imp)(
            ShellRequest::NewShellSurface {
                surface: make_handle(&shell_surface),
            },
            data,
        );
    });
}

fn make_handle(shell_surface: &wl_shell_surface::WlShellSurface) -> ShellSurface {
    let data = shell_surface
        .as_ref()
        .user_data()
        .get::<ShellSurfaceUserData>()
        .unwrap();
    ShellSurface {
        wl_surface: data.surface.clone(),
        shell_surface: shell_surface.clone(),
    }
}

pub(crate) struct ShellSurfaceUserData {
    surface: wl_surface::WlSurface,
    state: Arc<Mutex<ShellState>>,
}

fn implement_shell_surface<Impl>(
    shell_surface: Main<wl_shell_surface::WlShellSurface>,
    surface: wl_surface::WlSurface,
    implementation: Rc<RefCell<Impl>>,
    state: Arc<Mutex<ShellState>>,
) -> wl_shell_surface::WlShellSurface
where
    Impl: FnMut(ShellRequest, DispatchData<'_>) + 'static,
{
    use self::wl_shell_surface::Request;
    shell_surface.quick_assign(move |shell_surface, req, dispatch_data| {
        let data = shell_surface
            .as_ref()
            .user_data()
            .get::<ShellSurfaceUserData>()
            .unwrap();
        let mut user_impl = implementation.borrow_mut();
        match req {
            Request::Pong { serial } => {
                let serial = Serial::from(serial);
                let valid = compositor::with_states(&data.surface, |states| {
                    let mut guard = states
                        .data_map
                        .get::<Mutex<ShellSurfaceAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap();
                    if guard.pending_ping == Some(serial) {
                        guard.pending_ping = None;
                        true
                    } else {
                        false
                    }
                })
                .unwrap();
                if valid {
                    (&mut *user_impl)(
                        ShellRequest::Pong {
                            surface: make_handle(&shell_surface),
                        },
                        dispatch_data,
                    );
                }
            }
            Request::Move { seat, serial } => {
                let serial = Serial::from(serial);
                (&mut *user_impl)(
                    ShellRequest::Move {
                        surface: make_handle(&shell_surface),
                        serial,
                        seat,
                    },
                    dispatch_data,
                )
            }
            Request::Resize { seat, serial, edges } => {
                let serial = Serial::from(serial);
                (&mut *user_impl)(
                    ShellRequest::Resize {
                        surface: make_handle(&shell_surface),
                        serial,
                        seat,
                        edges,
                    },
                    dispatch_data,
                )
            }
            Request::SetToplevel => (&mut *user_impl)(
                ShellRequest::SetKind {
                    surface: make_handle(&shell_surface),
                    kind: ShellSurfaceKind::Toplevel,
                },
                dispatch_data,
            ),
            Request::SetTransient { parent, x, y, flags } => (&mut *user_impl)(
                ShellRequest::SetKind {
                    surface: make_handle(&shell_surface),
                    kind: ShellSurfaceKind::Transient {
                        parent,
                        location: (x, y),
                        inactive: flags.contains(wl_shell_surface::Transient::Inactive),
                    },
                },
                dispatch_data,
            ),
            Request::SetFullscreen {
                method,
                framerate,
                output,
            } => (&mut *user_impl)(
                ShellRequest::SetKind {
                    surface: make_handle(&shell_surface),
                    kind: ShellSurfaceKind::Fullscreen {
                        method,
                        framerate,
                        output,
                    },
                },
                dispatch_data,
            ),
            Request::SetPopup {
                seat,
                serial,
                parent,
                x,
                y,
                flags,
            } => {
                let serial = Serial::from(serial);
                (&mut *user_impl)(
                    ShellRequest::SetKind {
                        surface: make_handle(&shell_surface),
                        kind: ShellSurfaceKind::Popup {
                            parent,
                            serial,
                            seat,
                            location: (x, y),
                            inactive: flags.contains(wl_shell_surface::Transient::Inactive),
                        },
                    },
                    dispatch_data,
                )
            }
            Request::SetMaximized { output } => (&mut *user_impl)(
                ShellRequest::SetKind {
                    surface: make_handle(&shell_surface),
                    kind: ShellSurfaceKind::Maximized { output },
                },
                dispatch_data,
            ),
            Request::SetTitle { title } => {
                compositor::with_states(&data.surface, |states| {
                    let mut guard = states
                        .data_map
                        .get::<Mutex<ShellSurfaceAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap();
                    guard.title = title;
                })
                .unwrap();
            }
            Request::SetClass { class_ } => {
                compositor::with_states(&data.surface, |states| {
                    let mut guard = states
                        .data_map
                        .get::<Mutex<ShellSurfaceAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap();
                    guard.class = class_;
                })
                .unwrap();
            }
            _ => unreachable!(),
        }
    });

    shell_surface.assign_destructor(Filter::new(
        |shell_surface: wl_shell_surface::WlShellSurface, _, _data| {
            let data = shell_surface
                .as_ref()
                .user_data()
                .get::<ShellSurfaceUserData>()
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
