mod compositor;
mod xdg_shell;

use crate::Smallvil;

//
// Wl Seat
//

use smithay::wayland::data_device::{ClientDndGrabHandler, DataDeviceHandler, ServerDndGrabHandler};
use smithay::wayland::seat::{SeatHandler, SeatState};
use smithay::{delegate_data_device, delegate_seat};

use wayland_server::{delegate_dispatch, delegate_global_dispatch};

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

use smithay::wayland::output::OutputManagerState;
use wayland_protocols::unstable::xdg_output::v1::server::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1, zxdg_output_v1::ZxdgOutputV1,
};
use wayland_server::protocol::wl_output::WlOutput;

// Wl Output
delegate_global_dispatch!(Smallvil: [WlOutput, ZxdgOutputManagerV1] => OutputManagerState);
delegate_dispatch!(Smallvil: [WlOutput, ZxdgOutputManagerV1, ZxdgOutputV1] => OutputManagerState);
