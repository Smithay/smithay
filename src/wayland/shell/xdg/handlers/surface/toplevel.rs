use std::sync::Mutex;

use crate::wayland::delegate::{DelegateDispatch, DelegateDispatchBase};
use crate::wayland::{compositor, Serial};
use wayland_protocols::xdg_shell::server::xdg_toplevel::{self, XdgToplevel};
use wayland_server::protocol::wl_surface;
use wayland_server::{DataInit, Dispatch, DisplayHandle, Resource, WEnum};

use super::{
    SurfaceCachedState, ToplevelConfigure, XdgRequest, XdgShellDispatch, XdgShellHandler,
    XdgShellSurfaceUserData, XdgToplevelSurfaceRoleAttributes,
};

impl<D, H: XdgShellHandler<D>> DelegateDispatchBase<XdgToplevel> for XdgShellDispatch<'_, D, H> {
    type UserData = XdgShellSurfaceUserData;
}

impl<D, H> DelegateDispatch<XdgToplevel, D> for XdgShellDispatch<'_, D, H>
where
    D: Dispatch<XdgToplevel, UserData = XdgShellSurfaceUserData> + 'static,
    H: XdgShellHandler<D>,
{
    fn request(
        &mut self,
        _client: &wayland_server::Client,
        toplevel: &XdgToplevel,
        request: xdg_toplevel::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, D>,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_toplevel::Request::Destroy => {
                // all it done by the destructor
            }
            xdg_toplevel::Request::SetParent { parent } => {
                let parent_surface = parent.map(|toplevel_surface_parent| {
                    toplevel_surface_parent
                        .data::<XdgShellSurfaceUserData>()
                        .unwrap()
                        .wl_surface
                        .clone()
                });

                // Parent is not double buffered, we can set it directly
                set_parent::<D>(&toplevel, parent_surface);
            }
            xdg_toplevel::Request::SetTitle { title } => {
                // Title is not double buffered, we can set it directly
                with_surface_toplevel_role_data::<D, _, _>(&toplevel, |data| {
                    data.title = Some(title);
                });
            }
            xdg_toplevel::Request::SetAppId { app_id } => {
                // AppId is not double buffered, we can set it directly
                with_surface_toplevel_role_data::<D, _, _>(&toplevel, |role| {
                    role.app_id = Some(app_id);
                });
            }
            xdg_toplevel::Request::ShowWindowMenu { seat, serial, x, y } => {
                // This has to be handled by the compositor
                let handle = make_toplevel_handle(&toplevel);
                let serial = Serial::from(serial);

                self.1.request(
                    cx,
                    XdgRequest::ShowWindowMenu {
                        surface: handle,
                        seat,
                        serial,
                        location: (x, y).into(),
                    },
                );
            }
            xdg_toplevel::Request::Move { seat, serial } => {
                // This has to be handled by the compositor
                let handle = make_toplevel_handle(&toplevel);
                let serial = Serial::from(serial);

                self.1.request(
                    cx,
                    XdgRequest::Move {
                        surface: handle,
                        seat,
                        serial,
                    },
                );
            }
            xdg_toplevel::Request::Resize { seat, serial, edges } => {
                if let WEnum::Value(edges) = edges {
                    // This has to be handled by the compositor
                    let handle = make_toplevel_handle(&toplevel);
                    let serial = Serial::from(serial);

                    self.1.request(
                        cx,
                        XdgRequest::Resize {
                            surface: handle,
                            seat,
                            serial,
                            edges,
                        },
                    );
                }
            }
            xdg_toplevel::Request::SetMaxSize { width, height } => {
                with_toplevel_pending_state::<D, _, _>(data, |toplevel_data| {
                    toplevel_data.max_size = (width, height).into();
                });
            }
            xdg_toplevel::Request::SetMinSize { width, height } => {
                with_toplevel_pending_state::<D, _, _>(data, |toplevel_data| {
                    toplevel_data.min_size = (width, height).into();
                });
            }
            xdg_toplevel::Request::SetMaximized => {
                let handle = make_toplevel_handle(&toplevel);
                self.1.request(cx, XdgRequest::Maximize { surface: handle });
            }
            xdg_toplevel::Request::UnsetMaximized => {
                let handle = make_toplevel_handle(&toplevel);
                self.1.request(cx, XdgRequest::UnMaximize { surface: handle });
            }
            xdg_toplevel::Request::SetFullscreen { output } => {
                let handle = make_toplevel_handle(&toplevel);
                self.1.request(
                    cx,
                    XdgRequest::Fullscreen {
                        surface: handle,
                        output,
                    },
                );
            }
            xdg_toplevel::Request::UnsetFullscreen => {
                let handle = make_toplevel_handle(&toplevel);
                self.1.request(cx, XdgRequest::UnFullscreen { surface: handle });
            }
            xdg_toplevel::Request::SetMinimized => {
                // This has to be handled by the compositor, may not be
                // supported and just ignored
                let handle = make_toplevel_handle(&toplevel);
                self.1.request(cx, XdgRequest::Minimize { surface: handle });
            }
            _ => unreachable!(),
        }
    }
}

// Utility functions allowing to factor out a lot of the upcoming logic
fn with_surface_toplevel_role_data<D, F, T>(toplevel: &xdg_toplevel::XdgToplevel, f: F) -> T
where
    F: FnOnce(&mut XdgToplevelSurfaceRoleAttributes) -> T,
    D: 'static,
{
    let data = toplevel.data::<XdgShellSurfaceUserData>().unwrap();
    compositor::with_states::<D, _, _>(&data.wl_surface, |states| {
        f(&mut *states
            .data_map
            .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
            .unwrap()
            .lock()
            .unwrap())
    })
    .unwrap()
}

pub(super) fn make_toplevel_handle(
    resource: &xdg_toplevel::XdgToplevel,
) -> crate::wayland::shell::xdg::ToplevelSurface {
    let data = resource.data::<XdgShellSurfaceUserData>().unwrap();
    crate::wayland::shell::xdg::ToplevelSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: resource.clone(),
    }
}

pub fn get_parent<D: 'static>(toplevel: &xdg_toplevel::XdgToplevel) -> Option<wl_surface::WlSurface> {
    with_surface_toplevel_role_data::<D, _, _>(toplevel, |data| data.parent.clone())
}

/// Sets the parent of the specified toplevel surface.
///
/// The parent must be a toplevel surface.
///
/// The parent of a surface is not double buffered and therefore may be set directly.
///
/// If the parent is `None`, the parent-child relationship is removed.
pub fn set_parent<D: 'static>(toplevel: &xdg_toplevel::XdgToplevel, parent: Option<wl_surface::WlSurface>) {
    with_surface_toplevel_role_data::<D, _, _>(toplevel, |data| {
        data.parent = parent;
    });
}

fn with_toplevel_pending_state<D, F, T>(data: &XdgShellSurfaceUserData, f: F) -> T
where
    F: FnOnce(&mut SurfaceCachedState) -> T,
    D: 'static,
{
    compositor::with_states::<D, _, _>(&data.wl_surface, |states| {
        f(&mut *states.cached_state.pending::<SurfaceCachedState>())
    })
    .unwrap()
}

pub fn send_toplevel_configure<D>(
    cx: &mut DisplayHandle<'_, D>,
    resource: &xdg_toplevel::XdgToplevel,
    configure: ToplevelConfigure,
) {
    let data = resource.data::<XdgShellSurfaceUserData>().unwrap();

    let (width, height) = configure.state.size.unwrap_or_default().into();
    // convert the Vec<State> (which is really a Vec<u32>) into Vec<u8>
    let states = {
        let mut states: Vec<xdg_toplevel::State> =
            configure.state.states.into_filtered_states(resource.version());
        let ptr = states.as_mut_ptr();
        let len = states.len();
        let cap = states.capacity();
        ::std::mem::forget(states);
        unsafe { Vec::from_raw_parts(ptr as *mut u8, len * 4, cap * 4) }
    };
    let serial = configure.serial;

    // Send the toplevel configure
    resource.configure(cx, width, height, states);

    // Send the base xdg_surface configure event to mark
    // The configure as finished
    data.xdg_surface.configure(cx, serial.into());
}
