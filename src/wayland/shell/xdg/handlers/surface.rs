use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use crate::utils::alive_tracker::{AliveTracker, IsAlive};
use crate::{
    utils::Rectangle,
    wayland::{
        compositor,
        shell::xdg::{PopupState, XdgShellState, XDG_POPUP_ROLE, XDG_TOPLEVEL_ROLE},
        Serial,
    },
};

use wayland_protocols::{
    xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1,
    xdg::shell::server::{
        xdg_popup::XdgPopup, xdg_surface, xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel, xdg_wm_base,
    },
};

use wayland_server::{protocol::wl_surface, DataInit, DelegateDispatch, Dispatch, DisplayHandle, Resource};

use super::{
    super::XdgSurfaceId, InnerState, PopupConfigure, SurfaceCachedState, ToplevelConfigure,
    XdgPopupSurfaceRoleAttributes, XdgPositionerUserData, XdgShellHandler, XdgToplevelSurfaceRoleAttributes,
};

mod toplevel;
use toplevel::make_toplevel_handle;
pub use toplevel::{get_parent, send_toplevel_configure, set_parent};

mod popup;
use popup::make_popup_handle;
pub use popup::send_popup_configure;

/// User data of XdgSurface
#[derive(Debug)]
pub struct XdgSurfaceUserData {
    pub(crate) wl_surface: wl_surface::WlSurface,
    pub(crate) wm_base: xdg_wm_base::XdgWmBase,
    pub(crate) has_active_role: AtomicBool,
}

impl<D> DelegateDispatch<XdgSurface, XdgSurfaceUserData, D> for XdgShellState
where
    D: Dispatch<XdgSurface, XdgSurfaceUserData>,
    D: Dispatch<XdgToplevel, XdgShellSurfaceUserData>,
    D: Dispatch<XdgPopup, XdgShellSurfaceUserData>,
    D: XdgShellHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        xdg_surface: &XdgSurface,
        request: xdg_surface::Request,
        data: &XdgSurfaceUserData,
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_surface::Request::Destroy => {
                if !data.wl_surface.alive() {
                    // the wl_surface is destroyed, this means the client is not
                    // trying to change the role but it's a cleanup (possibly a
                    // disconnecting client), ignore the protocol check.
                    return;
                }

                // reset the XdgSurfaceId
                compositor::with_states(&data.wl_surface, |states| {
                    states
                        .data_map
                        .get::<Mutex<XdgSurfaceId>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .0 = wayland_server::backend::Handle::null_id();
                });

                if compositor::get_role(&data.wl_surface).is_none() {
                    // No role assigned to the surface, we can exit early.
                    return;
                }

                if data.has_active_role.load(Ordering::Acquire) {
                    data.wm_base.post_error(
                        xdg_wm_base::Error::Role,
                        "xdg_surface was destroyed before its role object",
                    );
                }
            }
            xdg_surface::Request::GetToplevel { id } => {
                // We now can assign a role to the surface
                let surface = &data.wl_surface;
                let shell = &data.wm_base;

                if compositor::give_role(surface, XDG_TOPLEVEL_ROLE).is_err() {
                    shell.post_error(xdg_wm_base::Error::Role, "Surface already has a role.");
                    return;
                }

                data.has_active_role.store(true, Ordering::Release);

                compositor::with_states(surface, |states| {
                    states.data_map.insert_if_missing_threadsafe(|| {
                        Mutex::new(XdgToplevelSurfaceRoleAttributes::default())
                    })
                });

                compositor::add_pre_commit_hook(surface, super::super::ToplevelSurface::commit_hook);

                let toplevel = data_init.init(
                    id,
                    XdgShellSurfaceUserData {
                        shell_data: state.xdg_shell_state().inner.clone(),
                        wl_surface: data.wl_surface.clone(),
                        xdg_surface: xdg_surface.clone(),
                        wm_base: data.wm_base.clone(),
                        decoration: Default::default(),
                        alive_tracker: Default::default(),
                    },
                );

                state
                    .xdg_shell_state()
                    .inner
                    .lock()
                    .unwrap()
                    .known_toplevels
                    .push(make_toplevel_handle(&toplevel));

                let handle = make_toplevel_handle(&toplevel);

                XdgShellHandler::new_toplevel(state, dh, handle);
            }
            xdg_surface::Request::GetPopup {
                id,
                parent,
                positioner,
            } => {
                let positioner_data = *positioner
                    .data::<XdgPositionerUserData>()
                    .unwrap()
                    .inner
                    .lock()
                    .unwrap();

                let parent_surface = parent.map(|parent| {
                    let parent_data = parent.data::<XdgSurfaceUserData>().unwrap();
                    parent_data.wl_surface.clone()
                });

                // We now can assign a role to the surface
                let surface = &data.wl_surface;
                let shell = &data.wm_base;

                let attributes = XdgPopupSurfaceRoleAttributes {
                    parent: parent_surface,
                    server_pending: Some(PopupState {
                        // Set the positioner data as the popup geometry
                        geometry: positioner_data.get_geometry(),
                        positioner: positioner_data,
                    }),
                    ..Default::default()
                };
                if compositor::give_role(surface, XDG_POPUP_ROLE).is_err() {
                    shell.post_error(xdg_wm_base::Error::Role, "Surface already has a role.");
                    return;
                }

                data.has_active_role.store(true, Ordering::Release);

                compositor::with_states(surface, |states| {
                    states.data_map.insert_if_missing_threadsafe(|| {
                        Mutex::new(XdgPopupSurfaceRoleAttributes::default())
                    });
                    *states
                        .data_map
                        .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap() = attributes;
                });

                compositor::add_pre_commit_hook(surface, super::super::PopupSurface::commit_hook);

                let popup = data_init.init(
                    id,
                    XdgShellSurfaceUserData {
                        shell_data: state.xdg_shell_state().inner.clone(),
                        wl_surface: data.wl_surface.clone(),
                        xdg_surface: xdg_surface.clone(),
                        wm_base: data.wm_base.clone(),
                        decoration: Default::default(),
                        alive_tracker: Default::default(),
                    },
                );

                state
                    .xdg_shell_state()
                    .inner
                    .lock()
                    .unwrap()
                    .known_popups
                    .push(make_popup_handle(&popup));

                let handle = make_popup_handle(&popup);

                XdgShellHandler::new_popup(state, dh, handle, positioner_data);
            }
            xdg_surface::Request::SetWindowGeometry { x, y, width, height } => {
                // Check the role of the surface, this can be either xdg_toplevel
                // or xdg_popup. If none of the role matches the xdg_surface has no role set
                // which is a protocol error.
                let surface = &data.wl_surface;

                let role = compositor::get_role(surface);

                if role.is_none() {
                    xdg_surface.post_error(
                        xdg_surface::Error::NotConstructed,
                        "xdg_surface must have a role.",
                    );
                    return;
                }

                if role != Some(XDG_TOPLEVEL_ROLE) && role != Some(XDG_POPUP_ROLE) {
                    data.wm_base.post_error(
                        xdg_wm_base::Error::Role,
                        "xdg_surface must have a role of xdg_toplevel or xdg_popup.",
                    );
                }

                compositor::with_states(surface, |states| {
                    states.cached_state.pending::<SurfaceCachedState>().geometry =
                        Some(Rectangle::from_loc_and_size((x, y), (width, height)));
                });
            }
            xdg_surface::Request::AckConfigure { serial } => {
                let serial = Serial::from(serial);
                let surface = &data.wl_surface;

                // Check the role of the surface, this can be either xdg_toplevel
                // or xdg_popup. If none of the role matches the xdg_surface has no role set
                // which is a protocol error.
                if compositor::get_role(surface).is_none() {
                    xdg_surface.post_error(
                        xdg_surface::Error::NotConstructed,
                        "xdg_surface must have a role.",
                    );
                    return;
                }

                // Find the correct configure state for the provided serial
                // discard all configure states that are older than the provided
                // serial.
                // If no matching serial can be found raise a protocol error
                //
                // Invoke the user impl with the found configuration
                // This has to include the serial and the role specific data.
                // - For xdg_popup there is no data.
                // - For xdg_toplevel send the state data including
                //   width, height, min/max size, maximized, fullscreen, resizing, activated
                //
                // This can be used to integrate custom protocol extensions
                let found_configure = compositor::with_states(surface, |states| {
                    if states.role == Some(XDG_TOPLEVEL_ROLE) {
                        Ok(states
                            .data_map
                            .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .ack_configure(serial))
                    } else if states.role == Some(XDG_POPUP_ROLE) {
                        Ok(states
                            .data_map
                            .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .ack_configure(serial))
                    } else {
                        Err(())
                    }
                });

                let configure = match found_configure {
                    Ok(Some(configure)) => configure,
                    Ok(None) => {
                        data.wm_base.post_error(
                            xdg_wm_base::Error::InvalidSurfaceState,
                            format!("wrong configure serial: {}", <u32>::from(serial)),
                        );
                        return;
                    }
                    Err(()) => {
                        data.wm_base.post_error(
                            xdg_wm_base::Error::Role as u32,
                            "xdg_surface must have a role of xdg_toplevel or xdg_popup.",
                        );
                        return;
                    }
                };

                XdgShellHandler::ack_configure(state, dh, surface.clone(), configure);
            }
            _ => unreachable!(),
        }
    }
}

/// User data of xdg toplevel surface
#[derive(Debug)]
pub struct XdgShellSurfaceUserData {
    pub(crate) shell_data: Arc<Mutex<InnerState>>,
    pub(crate) wl_surface: wl_surface::WlSurface,
    pub(crate) wm_base: xdg_wm_base::XdgWmBase,
    pub(crate) xdg_surface: xdg_surface::XdgSurface,
    pub(crate) decoration: Mutex<Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>>,

    pub(crate) alive_tracker: AliveTracker,
}

impl IsAlive for XdgToplevel {
    fn alive(&self) -> bool {
        let data: &XdgShellSurfaceUserData = self.data().unwrap();
        data.alive_tracker.alive()
    }
}

impl IsAlive for XdgPopup {
    fn alive(&self) -> bool {
        let data: &XdgShellSurfaceUserData = self.data().unwrap();
        data.alive_tracker.alive()
    }
}
