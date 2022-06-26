use smithay::{
    delegate_xdg_shell,
    desktop::{Kind, Window, WindowSurfaceType},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            protocol::{wl_seat, wl_surface::WlSurface},
            DisplayHandle, Resource,
        },
    },
    utils::Rectangle,
    wayland::{
        seat::{Focus, PointerGrabStartData, Seat},
        shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState},
        Serial,
    },
};

use crate::{
    grabs::{MoveSurfaceGrab, ResizeSurfaceGrab},
    Smallvil,
};

impl XdgShellHandler for Smallvil {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, _dh: &DisplayHandle, surface: ToplevelSurface) {
        let window = Window::new(Kind::Xdg(surface.clone()));
        self.space.map_window(&window, (0, 0), None, false);

        surface.send_configure();
    }
    fn new_popup(&mut self, _dh: &DisplayHandle, _surface: PopupSurface, _positioner: PositionerState) {}

    fn move_request(
        &mut self,
        _dh: &DisplayHandle,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
    ) {
        let seat = Seat::from_resource(&seat).unwrap();

        let wl_surface = surface.wl_surface();

        if let Some(start_data) = check_grab(&seat, wl_surface, serial) {
            let pointer = seat.get_pointer().unwrap();

            let window = self
                .space
                .window_for_surface(wl_surface, WindowSurfaceType::TOPLEVEL)
                .unwrap()
                .clone();
            let initial_window_location = self.space.window_location(&window).unwrap();

            let grab = MoveSurfaceGrab {
                start_data,
                window,
                initial_window_location,
            };

            pointer.set_grab(grab, serial, Focus::Clear);
        }
    }

    fn resize_request(
        &mut self,
        _dh: &DisplayHandle,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let seat = Seat::from_resource(&seat).unwrap();

        let wl_surface = surface.wl_surface();

        if let Some(start_data) = check_grab(&seat, wl_surface, serial) {
            let pointer = seat.get_pointer().unwrap();

            let window = self
                .space
                .window_for_surface(wl_surface, WindowSurfaceType::TOPLEVEL)
                .unwrap()
                .clone();
            let initial_window_location = self.space.window_location(&window).unwrap();
            let initial_window_size = window.geometry().size;

            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Resizing);
            });

            surface.send_configure();

            let grab = ResizeSurfaceGrab::start(
                start_data,
                window,
                edges.into(),
                Rectangle::from_loc_and_size(initial_window_location, initial_window_size),
            );

            pointer.set_grab(grab, serial, Focus::Clear);
        }
    }

    fn grab(&mut self, _dh: &DisplayHandle, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // TODO popup grabs
    }
}

// Xdg Shell
delegate_xdg_shell!(Smallvil);

fn check_grab(seat: &Seat<Smallvil>, surface: &WlSurface, serial: Serial) -> Option<PointerGrabStartData> {
    let pointer = seat.get_pointer()?;

    // Check that this surface has a click grab.
    if !pointer.has_grab(serial) {
        return None;
    }

    let start_data = pointer.grab_start_data()?;

    let (focus, _) = start_data.focus.as_ref()?;
    // If the focus was for a different surface, ignore the request.
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }

    Some(start_data)
}
