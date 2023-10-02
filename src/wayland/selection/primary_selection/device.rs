use std::cell::RefCell;

use tracing::debug;
use wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_v1::{
    self as primary_device, ZwpPrimarySelectionDeviceV1 as PrimaryDevice,
};
use wayland_server::{protocol::wl_seat::WlSeat, Client, DataInit, Dispatch, DisplayHandle, Resource};

use crate::{
    input::{Seat, SeatHandler},
    wayland::{
        seat::WaylandFocus,
        selection::{
            device::SelectionDevice,
            offer::OfferReplySource,
            seat_data::SeatData,
            source::{SelectionSource, SelectionSourceProvider},
            SelectionHandler, SelectionTarget,
        },
    },
};

use super::PrimarySelectionState;

#[doc(hidden)]
#[derive(Debug)]
pub struct PrimaryDeviceUserData {
    pub(crate) wl_seat: WlSeat,
}

impl<D> Dispatch<PrimaryDevice, PrimaryDeviceUserData, D> for PrimarySelectionState
where
    D: Dispatch<PrimaryDevice, PrimaryDeviceUserData>,
    D: SelectionHandler,
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
        let seat = match Seat::<D>::from_resource(&data.wl_seat) {
            Some(seat) => seat,
            None => return,
        };

        match request {
            primary_device::Request::SetSelection { source, .. } => {
                let seat_data = match seat.get_keyboard() {
                    Some(keyboard) if keyboard.client_of_object_has_focus(&resource.id()) => seat
                        .user_data()
                        .get::<RefCell<SeatData<D::SelectionUserData>>>()
                        .unwrap(),
                    _ => {
                        debug!(
                            client = ?client,
                            "denying setting selection by a non-focused client"
                        );
                        return;
                    }
                };

                let source = source.map(SelectionSourceProvider::Primary);

                handler.new_selection(
                    SelectionTarget::Primary,
                    source.clone().map(|provider| SelectionSource { provider }),
                    seat.clone(),
                );

                // The client has kbd focus, it can set the selection
                seat_data
                    .borrow_mut()
                    .set_primary_selection::<D>(dh, source.map(OfferReplySource::Client));
            }
            primary_device::Request::Destroy => seat
                .user_data()
                .get::<RefCell<SeatData<D::SelectionUserData>>>()
                .unwrap()
                .borrow_mut()
                .retain_devices(|ndd| match ndd {
                    SelectionDevice::Primary(ndd) => ndd != resource,
                    _ => true,
                }),
            _ => unreachable!(),
        }
    }
}
