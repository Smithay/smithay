use std::sync::{atomic::AtomicBool, Arc, Mutex};

use indexmap::IndexSet;

use crate::{
    utils::{alive_tracker::AliveTracker, IsAlive, Serial},
    wayland::shell::xdg::XdgShellState,
};

use wayland_protocols::xdg::shell::server::{
    xdg_positioner::XdgPositioner, xdg_surface, xdg_surface::XdgSurface, xdg_wm_base, xdg_wm_base::XdgWmBase,
};

use wayland_server::{
    backend::ClientId, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak,
};

use super::{ShellClient, ShellClientData, XdgPositionerUserData, XdgShellHandler, XdgSurfaceUserData};

impl<D> GlobalDispatch<XdgWmBase, (), D> for XdgShellState
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
        _dh: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<XdgWmBase>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        let shell = data_init.init(resource, XdgWmBaseUserData::default());

        XdgShellHandler::new_client(state, ShellClient::new(&shell));
    }
}

impl<D> Dispatch<XdgWmBase, XdgWmBaseUserData, D> for XdgShellState
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
        _dh: &DisplayHandle,
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
                let xdg_surface = data_init.init(
                    id,
                    XdgSurfaceUserData {
                        known_surfaces: data.known_surfaces.clone(),
                        wl_surface: surface,
                        wm_base: wm_base.clone(),
                        has_active_role: AtomicBool::new(false),
                    },
                );
                data.known_surfaces
                    .lock()
                    .unwrap()
                    .insert(xdg_surface.downgrade());
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
                    XdgShellHandler::client_pong(state, ShellClient::new(wm_base));
                }
            }
            xdg_wm_base::Request::Destroy => {
                if !data.known_surfaces.lock().unwrap().is_empty() {
                    wm_base.post_error(
                        xdg_wm_base::Error::DefunctSurfaces,
                        "xdg_wm_base was destroyed before children",
                    );
                }
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(state: &mut D, _client_id: ClientId, wm_base: &XdgWmBase, data: &XdgWmBaseUserData) {
        XdgShellHandler::client_destroyed(state, ShellClient::new(wm_base));
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
    known_surfaces: Arc<Mutex<IndexSet<Weak<xdg_surface::XdgSurface>>>>,
    alive_tracker: AliveTracker,
}
