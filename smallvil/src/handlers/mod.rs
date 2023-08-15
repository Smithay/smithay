mod compositor;
mod xdg_shell;

use crate::Smallvil;

//
// Wl Seat
//

use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::data_device::{
    set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, ServerDndGrabHandler,
};
use smithay::{delegate_data_device, delegate_output, delegate_seat};

impl SeatHandler for Smallvil {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Smallvil> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: smithay::input::pointer::CursorImageStatus) {}

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}

delegate_seat!(Smallvil);

//
// Wl Data Device
//

impl DataDeviceHandler for Smallvil {
    type SelectionUserData = ();
    fn data_device_state(&self) -> &smithay::wayland::data_device::DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Smallvil {}
impl ServerDndGrabHandler for Smallvil {}

delegate_data_device!(Smallvil);

//
// Wl Output & Xdg Output
//

delegate_output!(Smallvil);
