use std::cell::RefCell;

use tracing::debug;
use wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_device_v1::{
    self as primary_device, ZwpPrimarySelectionDeviceV1 as PrimaryDevice,
};
use wayland_server::{Client, DataInit, DisplayHandle, Resource, protocol::wl_seat::WlSeat};

use crate::{
    input::{Seat, SeatHandler},
    wayland::{
        Dispatch2,
        seat::WaylandFocus,
        selection::{
            SelectionHandler, SelectionTarget,
            device::SelectionDevice,
            offer::OfferReplySource,
            seat_data::SeatData,
            source::{SelectionSource, SelectionSourceProvider},
        },
    },
};

use super::PrimarySelectionHandler;

#[doc(hidden)]
#[derive(Debug)]
pub struct PrimaryDeviceUserData {
    pub(crate) wl_seat: WlSeat,
}

impl<D> Dispatch2<PrimaryDevice, D> for PrimaryDeviceUserData
where
    D: PrimarySelectionHandler,
    D: SelectionHandler,
    D: SeatHandler,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
    D: 'static,
{
    fn request(
        &self,
        handler: &mut D,
        client: &Client,
        resource: &PrimaryDevice,
        request: primary_device::Request,
        dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        let seat = match Seat::<D>::from_resource(&self.wl_seat) {
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

                // NOTE: While protocol states that selection shouldn't be used more than once,
                // no-one enforces it, thus we have clients around that do so and crashing them
                // doesn't worth it at this point.
                if let Some(source) = source.as_ref() {
                    handler
                        .primary_selection_state()
                        .used_sources
                        .insert(source.clone(), self.wl_seat.clone());
                }

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

    fn destroyed(
        &self,
        _state: &mut D,
        _client: wayland_server::backend::ClientId,
        resource: &PrimaryDevice,
    ) {
        if let Some(seat) = Seat::<D>::from_resource(&self.wl_seat) {
            if let Some(seat_data) = seat.user_data().get::<RefCell<SeatData<D::SelectionUserData>>>() {
                seat_data.borrow_mut().retain_devices(|ndd| match ndd {
                    SelectionDevice::Primary(ndd) => ndd != resource,
                    _ => true,
                });
            }
        }
    }
}
