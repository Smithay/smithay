use crate::wayland::delegate::{DelegateDispatch, DelegateDispatchBase};
use crate::wayland::shell::xdg::XdgPositionerUserData;
use crate::wayland::Serial;
use wayland_protocols::xdg_shell::server::xdg_popup;
use wayland_protocols::xdg_shell::server::xdg_popup::XdgPopup;
use wayland_server::{DataInit, Dispatch, DisplayHandle, Resource};

use super::{PopupConfigure, XdgRequest, XdgShellDispatch, XdgShellHandler, XdgShellSurfaceUserData};

impl<D, H: XdgShellHandler<D>> DelegateDispatchBase<XdgPopup> for XdgShellDispatch<'_, D, H> {
    type UserData = XdgShellSurfaceUserData;
}

impl<D, H> DelegateDispatch<XdgPopup, D> for XdgShellDispatch<'_, D, H>
where
    D: Dispatch<XdgPopup, UserData = XdgShellSurfaceUserData> + 'static,
    H: XdgShellHandler<D>,
{
    fn request(
        &mut self,
        _client: &wayland_server::Client,
        popup: &XdgPopup,
        request: xdg_popup::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, D>,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_popup::Request::Destroy => {
                // all is handled by our destructor
            }
            xdg_popup::Request::Grab { seat, serial } => {
                let handle = crate::wayland::shell::xdg::PopupSurface {
                    wl_surface: data.wl_surface.clone(),
                    shell_surface: popup.clone(),
                };

                let serial = Serial::from(serial);

                self.1.request(
                    cx,
                    XdgRequest::Grab {
                        surface: handle,
                        seat,
                        serial,
                    },
                );
            }
            xdg_popup::Request::Reposition { positioner, token } => {
                let handle = crate::wayland::shell::xdg::PopupSurface {
                    wl_surface: data.wl_surface.clone(),
                    shell_surface: popup.clone(),
                };

                let positioner_data = *positioner
                    .data::<XdgPositionerUserData>()
                    .unwrap()
                    .inner
                    .lock()
                    .unwrap();

                self.1.request(
                    cx,
                    XdgRequest::RePosition {
                        surface: handle,
                        positioner: positioner_data,
                        token,
                    },
                );
            }
            _ => unreachable!(),
        }
    }
}

pub fn send_popup_configure<D>(
    cx: &mut DisplayHandle<'_, D>,
    resource: &xdg_popup::XdgPopup,
    configure: PopupConfigure,
) {
    let data = resource.data::<XdgShellSurfaceUserData>().unwrap();

    let serial = configure.serial;
    let geometry = configure.state.geometry;

    // Send repositioned if token is set
    if let Some(token) = configure.reposition_token {
        resource.repositioned(cx, token);
    }

    // Send the popup configure
    resource.configure(
        cx,
        geometry.loc.x,
        geometry.loc.y,
        geometry.size.w,
        geometry.size.h,
    );

    // Send the base xdg_surface configure event to mark
    // the configure as finished
    data.xdg_surface.configure(cx, serial.into());
}

pub fn make_popup_handle(resource: &XdgPopup) -> crate::wayland::shell::xdg::PopupSurface {
    let data = resource.data::<XdgShellSurfaceUserData>().unwrap();
    crate::wayland::shell::xdg::PopupSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: resource.clone(),
    }
}
