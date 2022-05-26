mod compositor;
mod xdg_shell;

use crate::Smallvil;

//
// Wl Seat
//

use smithay::wayland::data_device::{ClientDndGrabHandler, DataDeviceHandler, ServerDndGrabHandler};
use smithay::wayland::seat::{SeatHandler, SeatState};
use smithay::{delegate_data_device, delegate_output, delegate_seat};

impl SeatHandler for Smallvil {
    fn seat_state(&mut self) -> &mut SeatState<Smallvil> {
        &mut self.seat_state
    }
}

delegate_seat!(Smallvil);

//
// Wl Data Device
//

impl DataDeviceHandler for Smallvil {
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
