use smithay::{
    delegate_xdg_shell,
    desktop::{Kind, Window},
    wayland::shell::xdg::{XdgRequest, XdgShellHandler, XdgShellState},
};
use wayland_server::{DisplayHandle, Resource};

use crate::{grabs::MoveSurfaceGrab, Smallvil};

impl XdgShellHandler for Smallvil {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn request(&mut self, dh: &mut DisplayHandle, request: XdgRequest) {
        match request {
            XdgRequest::NewToplevel { surface } => {
                let window = Window::new(Kind::Xdg(surface.clone()));
                self.space.map_window(&window, (0, 0), false);

                surface.send_configure(dh);
            }
            XdgRequest::Move { serial, surface, .. } => {
                // TODO: Multi seat support?
                // let seat = Seat::from_resource(&seat).unwrap();
                let seat = &mut self.seat;

                let wl_surface = surface.wl_surface();

                // TODO: touch move.
                let pointer = seat.get_pointer().unwrap();

                // Check that this surface has a click grab.
                if !pointer.has_grab(serial) {
                    return;
                }

                let start_data = pointer.grab_start_data().unwrap();

                // If the focus was for a different surface, ignore the request.
                if start_data.focus.is_none()
                    || !start_data
                        .focus
                        .as_ref()
                        .unwrap()
                        .0
                        .id()
                        .same_client_as(&wl_surface.id())
                {
                    return;
                }

                let window = self.space.window_for_surface(wl_surface).unwrap().clone();
                let initial_window_location = self.space.window_location(&window).unwrap();

                let grab = MoveSurfaceGrab {
                    start_data,
                    window,
                    initial_window_location,
                };

                pointer.set_grab(dh, grab, serial, 0);
            }
            _ => {}
        }
    }
}

// Xdg Shell
delegate_xdg_shell!(Smallvil);
