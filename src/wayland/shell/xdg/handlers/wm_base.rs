use std::sync::{Arc, Mutex, atomic::AtomicBool};

use indexmap::IndexSet;

use crate::{
    utils::{IsAlive, Serial, alive_tracker::AliveTracker},
    wayland::GlobalData,
};

use wayland_protocols::xdg::shell::server::{
    xdg_positioner::XdgPositioner, xdg_surface, xdg_surface::XdgSurface, xdg_wm_base, xdg_wm_base::XdgWmBase,
};

use wayland_server::{
    DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak, backend::ClientId,
};

use super::{ShellClient, ShellClientData, XdgPositionerUserData, XdgShellHandler, XdgSurfaceUserData};

impl<D> GlobalDispatch<XdgWmBase, D> for GlobalData
where
    D: XdgShellHandler,
    D: 'static,
{
    fn bind(
        &self,
        state: &mut D,
        _dh: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: New<XdgWmBase>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let shell = data_init.init(resource, XdgWmBaseUserData::default());

        XdgShellHandler::new_client(state, ShellClient::new(&shell));
    }
}

impl<D> Dispatch<XdgWmBase, D> for XdgWmBaseUserData
where
    D: XdgShellHandler,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &wayland_server::Client,
        wm_base: &XdgWmBase,
        request: xdg_wm_base::Request,
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
                        known_surfaces: self.known_surfaces.clone(),
                        wl_surface: surface,
                        wm_base: wm_base.clone(),
                        has_active_role: AtomicBool::new(false),
                    },
                );
                self.known_surfaces
                    .lock()
                    .unwrap()
                    .insert(xdg_surface.downgrade());
            }
            xdg_wm_base::Request::Pong { serial } => {
                let serial = Serial::from(serial);
                let valid = {
                    let mut guard = self.client_data.lock().unwrap();
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
                if !self.known_surfaces.lock().unwrap().is_empty() {
                    wm_base.post_error(
                        xdg_wm_base::Error::DefunctSurfaces,
                        "xdg_wm_base was destroyed before children",
                    );
                }
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(&self, state: &mut D, _client_id: ClientId, wm_base: &XdgWmBase) {
        XdgShellHandler::client_destroyed(state, ShellClient::new(wm_base));
        self.alive_tracker.destroy_notify();
    }
}

impl IsAlive for XdgWmBase {
    #[inline]
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
