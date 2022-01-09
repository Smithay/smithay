use crate::wayland::{shell::xdg::XdgPositionerUserData, Serial};

use wayland_protocols::xdg_shell::server::xdg_popup::{self, XdgPopup};

use wayland_server::{DataInit, DelegateDispatch, DelegateDispatchBase, Dispatch, DisplayHandle, Resource};

use super::{PopupConfigure, XdgRequest, XdgShellHandler, XdgShellState, XdgShellSurfaceUserData};

impl DelegateDispatchBase<XdgPopup> for XdgShellState {
    type UserData = XdgShellSurfaceUserData;
}

impl<D> DelegateDispatch<XdgPopup, D> for XdgShellState
where
    D: Dispatch<XdgPopup, UserData = XdgShellSurfaceUserData>,
    D: XdgShellHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        popup: &XdgPopup,
        request: xdg_popup::Request,
        data: &Self::UserData,
        dh: &mut DisplayHandle<'_>,
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

                XdgShellHandler::request(
                    state,
                    dh,
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

                XdgShellHandler::request(
                    state,
                    dh,
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

pub fn send_popup_configure(dh: &mut DisplayHandle<'_>, resource: &XdgPopup, configure: PopupConfigure) {
    let data = resource.data::<XdgShellSurfaceUserData>().unwrap();

    let serial = configure.serial;
    let geometry = configure.state.geometry;

    // Send repositioned if token is set
    if let Some(token) = configure.reposition_token {
        resource.repositioned(dh, token);
    }

    // Send the popup configure
    resource.configure(
        dh,
        geometry.loc.x,
        geometry.loc.y,
        geometry.size.w,
        geometry.size.h,
    );

    // Send the base xdg_surface configure event to mark
    // the configure as finished
    data.xdg_surface.configure(dh, serial.into());
}

pub fn make_popup_handle(resource: &XdgPopup) -> crate::wayland::shell::xdg::PopupSurface {
    let data = resource.data::<XdgShellSurfaceUserData>().unwrap();
    crate::wayland::shell::xdg::PopupSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: resource.clone(),
    }
}
