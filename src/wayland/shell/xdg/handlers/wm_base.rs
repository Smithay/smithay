use std::sync::atomic::AtomicBool;
use std::{ops::Deref as _, sync::Mutex};

use crate::wayland::delegate::{
    DelegateDispatch, DelegateDispatchBase, DelegateGlobalDispatch, DelegateGlobalDispatchBase,
};
use crate::wayland::Serial;
use wayland_protocols::xdg_shell::server::xdg_positioner::XdgPositioner;
use wayland_protocols::xdg_shell::server::xdg_surface::XdgSurface;
use wayland_protocols::xdg_shell::server::xdg_wm_base;
use wayland_protocols::xdg_shell::server::xdg_wm_base::XdgWmBase;
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::{DataInit, DestructionNotify, Dispatch, DisplayHandle, GlobalDispatch, New};

use super::{
    ShellClient, ShellClientData, XdgPositionerUserData, XdgRequest, XdgShellDispatch, XdgShellHandler,
    XdgSurfaceUserData,
};

impl<D, H: XdgShellHandler<D>> DelegateGlobalDispatchBase<XdgWmBase> for XdgShellDispatch<'_, D, H> {
    type GlobalData = ();
}

impl<D, H> DelegateGlobalDispatch<XdgWmBase, D> for XdgShellDispatch<'_, D, H>
where
    D: GlobalDispatch<XdgWmBase, GlobalData = ()>
        + Dispatch<XdgWmBase, UserData = XdgWmBaseUserData>
        + Dispatch<XdgSurface, UserData = XdgSurfaceUserData>
        + Dispatch<XdgPositioner, UserData = XdgPositionerUserData>
        + 'static,
    H: XdgShellHandler<D>,
{
    fn bind(
        &mut self,
        cx: &mut DisplayHandle<'_, D>,
        _client: &wayland_server::Client,
        resource: New<XdgWmBase>,
        _global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let shell = data_init.init(resource, XdgWmBaseUserData::default());

        self.1.request(
            cx,
            XdgRequest::NewClient {
                client: ShellClient::new(&shell),
            },
        );
    }
}

impl<D, H: XdgShellHandler<D>> DelegateDispatchBase<XdgWmBase> for XdgShellDispatch<'_, D, H> {
    type UserData = XdgWmBaseUserData;
}

impl<D, H> DelegateDispatch<XdgWmBase, D> for XdgShellDispatch<'_, D, H>
where
    D: Dispatch<XdgWmBase, UserData = XdgWmBaseUserData>
        + Dispatch<XdgSurface, UserData = XdgSurfaceUserData>
        + Dispatch<XdgPositioner, UserData = XdgPositionerUserData>
        + 'static,
    H: XdgShellHandler<D>,
{
    fn request(
        &mut self,
        _client: &wayland_server::Client,
        shell: &XdgWmBase,
        request: xdg_wm_base::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, D>,
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
                        wm_base: shell.deref().clone(),
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
                    self.1.request(
                        cx,
                        XdgRequest::ClientPong {
                            client: ShellClient::new(&shell),
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
