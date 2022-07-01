use std::sync::{atomic::AtomicBool, Mutex};

use crate::{
    utils::{alive_tracker::AliveTracker, IsAlive},
    wayland::{
        compositor::{self, SurfaceAttributes},
        shell::xdg::{XdgShellState, XDG_POPUP_ROLE, XDG_TOPLEVEL_ROLE},
        Serial,
    },
};

use wayland_protocols::xdg::shell::server::{
    xdg_positioner::XdgPositioner, xdg_surface::XdgSurface, xdg_wm_base, xdg_wm_base::XdgWmBase,
};

use wayland_server::{
    backend::{ClientId, ObjectId},
    DataInit, DelegateDispatch, DelegateGlobalDispatch, Dispatch, DisplayHandle, GlobalDispatch, New,
    Resource,
};

use super::{
    super::XdgSurfaceId, ShellClient, ShellClientData, XdgPositionerUserData, XdgShellHandler,
    XdgSurfaceUserData,
};

impl<D> DelegateGlobalDispatch<XdgWmBase, (), D> for XdgShellState
where
    D: GlobalDispatch<XdgWmBase, ()>,
    D: Dispatch<XdgWmBase, XdgWmBaseUserData>,
    D: Dispatch<XdgSurface, XdgSurfaceUserData>,
    D: Dispatch<XdgPositioner, XdgPositionerUserData>,
    D: XdgShellHandler,
    D: 'static,
{
    fn bind(
        state: &mut D,
        dh: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<XdgWmBase>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        let shell = data_init.init(resource, XdgWmBaseUserData::default());

        XdgShellHandler::new_client(state, dh, ShellClient::new(&shell));
    }
}

impl<D> DelegateDispatch<XdgWmBase, XdgWmBaseUserData, D> for XdgShellState
where
    D: Dispatch<XdgWmBase, XdgWmBaseUserData>,
    D: Dispatch<XdgSurface, XdgSurfaceUserData>,
    D: Dispatch<XdgPositioner, XdgPositionerUserData>,
    D: XdgShellHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        wm_base: &XdgWmBase,
        request: xdg_wm_base::Request,
        data: &XdgWmBaseUserData,
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_wm_base::Request::CreatePositioner { id } => {
                data_init.init(id, XdgPositionerUserData::default());
            }
            xdg_wm_base::Request::GetXdgSurface { id, surface } => {
                // Do not assign a role to the surface here
                // xdg_surface is not role, only xdg_toplevel and
                // xdg_popup are defined as roles

                // However, if the surface already has a role or an attached or commited buffer,
                // that is a problem
                compositor::with_states(&surface, |states| {
                    if !matches!(states.role, None | Some(XDG_TOPLEVEL_ROLE) | Some(XDG_POPUP_ROLE)) {
                        wm_base.post_error(xdg_wm_base::Error::Role, "surface already has a role");
                    }
                    let pending_state = states.cached_state.pending::<SurfaceAttributes>();
                    if pending_state.buffer.is_some() {
                        wm_base.post_error(
                            xdg_wm_base::Error::InvalidSurfaceState,
                            "surface has a buffer attached",
                        );
                    }
                    if pending_state.has_commited_buffer {
                        wm_base.post_error(
                            xdg_wm_base::Error::InvalidSurfaceState,
                            "surface has a buffer committed",
                        );
                    }
                    if let Some(mutex) = states.data_map.get::<Mutex<XdgSurfaceId>>() {
                        let guard = mutex.lock().unwrap();
                        if !guard.0.is_null() {
                            // there is already an xdg_surface associated with this object!
                            wm_base.post_error(
                                xdg_wm_base::Error::Role,
                                "and xdg_surface already exists for this surface",
                            );
                        }
                    }
                });

                // Creating an xdg_surface from a wl_surface with an attached or commited buffer is an error

                let xdg_surface = data_init.init(
                    id,
                    XdgSurfaceUserData {
                        wl_surface: surface.clone(),
                        wm_base: wm_base.clone(),
                        has_active_role: AtomicBool::new(false),
                    },
                );

                compositor::with_states(&surface, |states| {
                    let created = states
                        .data_map
                        .insert_if_missing_threadsafe(|| Mutex::new(XdgSurfaceId(xdg_surface.id())));
                    if !created {
                        states
                            .data_map
                            .get::<Mutex<XdgSurfaceId>>()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .0 = xdg_surface.id();
                    }
                })
            }
            xdg_wm_base::Request::Pong { serial } => {
                let serial = Serial::from(serial);
                let valid = {
                    let mut guard = data.client_data.lock().unwrap();
                    if guard.pending_ping == Some(serial) {
                        guard.pending_ping = None;
                        true
                    } else {
                        false
                    }
                };
                if valid {
                    XdgShellHandler::client_pong(state, dh, ShellClient::new(wm_base));
                }
            }
            xdg_wm_base::Request::Destroy => {
                // all is handled by destructor
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(_state: &mut D, _client_id: ClientId, _object_id: ObjectId, data: &XdgWmBaseUserData) {
        data.alive_tracker.destroy_notify();
    }
}

impl IsAlive for XdgWmBase {
    fn alive(&self) -> bool {
        let data: &XdgWmBaseUserData = self.data().unwrap();
        data.alive_tracker.alive()
    }
}

/*
 * xdg_shell
 */

/// User data for Xdg Wm Base
#[derive(Default, Debug)]
pub struct XdgWmBaseUserData {
    pub(crate) client_data: Mutex<ShellClientData>,
    alive_tracker: AliveTracker,
}
