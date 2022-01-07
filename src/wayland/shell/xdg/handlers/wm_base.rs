use std::sync::{atomic::AtomicBool, Mutex};

use crate::wayland::{shell::xdg::XdgShellState, Serial};

use wayland_protocols::xdg_shell::server::{
    xdg_positioner::XdgPositioner, xdg_surface::XdgSurface, xdg_wm_base, xdg_wm_base::XdgWmBase,
};

use wayland_server::{
    backend::{ClientId, ObjectId},
    DataInit, DelegateDispatch, DelegateDispatchBase, DelegateGlobalDispatch, DelegateGlobalDispatchBase,
    DestructionNotify, Dispatch, DisplayHandle, GlobalDispatch, New,
};

use super::{
    ShellClient, ShellClientData, XdgPositionerUserData, XdgRequest, XdgShellHandler, XdgSurfaceUserData,
};

impl DelegateGlobalDispatchBase<XdgWmBase> for XdgShellState {
    type GlobalData = ();
}

impl<D> DelegateGlobalDispatch<XdgWmBase, D> for XdgShellState
where
    D: GlobalDispatch<XdgWmBase, GlobalData = ()>,
    D: Dispatch<XdgWmBase, UserData = XdgWmBaseUserData>,
    D: Dispatch<XdgSurface, UserData = XdgSurfaceUserData>,
    D: Dispatch<XdgPositioner, UserData = XdgPositionerUserData>,
    D: XdgShellHandler,
    D: 'static,
{
    fn bind(
        state: &mut D,
        cx: &mut DisplayHandle<'_>,
        _client: &wayland_server::Client,
        resource: New<XdgWmBase>,
        _global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let shell = data_init.init(resource, XdgWmBaseUserData::default());

        XdgShellHandler::request(
            state,
            cx,
            XdgRequest::NewClient {
                client: ShellClient::new(&shell),
            },
        );
    }
}

impl DelegateDispatchBase<XdgWmBase> for XdgShellState {
    type UserData = XdgWmBaseUserData;
}

impl<D> DelegateDispatch<XdgWmBase, D> for XdgShellState
where
    D: Dispatch<XdgWmBase, UserData = XdgWmBaseUserData>,
    D: Dispatch<XdgSurface, UserData = XdgSurfaceUserData>,
    D: Dispatch<XdgPositioner, UserData = XdgPositionerUserData>,
    D: XdgShellHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        wm_base: &XdgWmBase,
        request: xdg_wm_base::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_>,
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

                data_init.init(
                    id,
                    XdgSurfaceUserData {
                        wl_surface: surface,
                        wm_base: wm_base.clone(),
                        has_active_role: AtomicBool::new(false),
                    },
                );
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
                    XdgShellHandler::request(
                        state,
                        cx,
                        XdgRequest::ClientPong {
                            client: ShellClient::new(wm_base),
                        },
                    );
                }
            }
            xdg_wm_base::Request::Destroy => {
                // all is handled by destructor
            }
            _ => unreachable!(),
        }
    }
}

/*
 * xdg_shell
 */

/// User data for Xdg Wm Base
#[derive(Default, Debug)]
pub struct XdgWmBaseUserData {
    pub(crate) client_data: Mutex<ShellClientData>,
}

impl DestructionNotify for XdgWmBaseUserData {
    fn object_destroyed(&self, _client_id: ClientId, _object_id: ObjectId) {}
}
