use std::cell::RefCell;

use tracing::debug;
use wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_v1::{
    self as primary_device, ZwpPrimarySelectionDeviceV1 as PrimaryDevice,
};
use wayland_server::{protocol::wl_seat::WlSeat, Client, DataInit, Dispatch, DisplayHandle, Resource};

use crate::{
    input::{Seat, SeatHandler},
    wayland::{
        primary_selection::seat_data::{SeatData, Selection},
        seat::WaylandFocus,
    },
};

use super::{PrimarySelectionHandler, PrimarySelectionState};

#[doc(hidden)]
#[derive(Debug)]
pub struct PrimaryDeviceUserData {
    pub(crate) wl_seat: WlSeat,
}

impl<D> Dispatch<PrimaryDevice, PrimaryDeviceUserData, D> for PrimarySelectionState
where
    D: Dispatch<PrimaryDevice, PrimaryDeviceUserData>,
    D: PrimarySelectionHandler,
    D: SeatHandler,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
    D: 'static,
{
    fn request(
        handler: &mut D,
        client: &Client,
        resource: &PrimaryDevice,
        request: primary_device::Request,
        data: &PrimaryDeviceUserData,
        dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        if let Some(seat) = Seat::<D>::from_resource(&data.wl_seat) {
            match request {
                primary_device::Request::SetSelection { source, .. } => {
                    if let Some(keyboard) = seat.get_keyboard() {
                        if keyboard.client_of_object_has_focus(&resource.id()) {
                            let seat_data = seat.user_data().get::<RefCell<SeatData>>().unwrap();

                            PrimarySelectionHandler::new_selection(handler, source.clone(), seat.clone());
                            // The client has kbd focus, it can set the selection
                            seat_data.borrow_mut().set_selection::<D>(
                                dh,
                                source.map(Selection::Client).unwrap_or(Selection::Empty),
                            );
                            return;
                        }
                    }
                    debug!(
                        client = ?client,
                        "denying setting selection by a non-focused client"
                    );
                }
                primary_device::Request::Destroy => {
                    // Clean up the known devices
                    seat.user_data()
                        .get::<RefCell<SeatData>>()
                        .unwrap()
                        .borrow_mut()
                        .retain_devices(|ndd| ndd != resource)
                }
                _ => unreachable!(),
            }
        }
    }
}
