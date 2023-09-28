use std::cell::RefCell;

use tracing::debug;
use wayland_server::{
    protocol::{
        wl_data_device::{self, WlDataDevice},
        wl_seat::WlSeat,
    },
    Client, DataInit, Dispatch, DisplayHandle, Resource,
};

use crate::{
    input::{pointer::Focus, Seat, SeatHandler},
    utils::Serial,
    wayland::{
        compositor,
        seat::WaylandFocus,
        selection::{
            device::SelectionDevice,
            offer::OfferReplySource,
            seat_data::SeatData,
            source::{SelectionSource, SelectionSourceProvider},
            SelectionTarget,
        },
    },
};

use super::{dnd_grab, DataDeviceHandler, DataDeviceState};

/// WlSurface role of drag and drop icon
pub const DND_ICON_ROLE: &str = "dnd_icon";

#[doc(hidden)]
#[derive(Debug)]
pub struct DataDeviceUserData {
    pub(crate) wl_seat: WlSeat,
}

impl<D> Dispatch<WlDataDevice, DataDeviceUserData, D> for DataDeviceState
where
    D: Dispatch<WlDataDevice, DataDeviceUserData>,
    D: DataDeviceHandler,
    D: SeatHandler,
    <D as SeatHandler>::PointerFocus: WaylandFocus,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
    D: 'static,
{
    fn request(
        handler: &mut D,
        client: &Client,
        resource: &WlDataDevice,
        request: wl_data_device::Request,
        data: &DataDeviceUserData,
        dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        let seat = match Seat::<D>::from_resource(&data.wl_seat) {
            Some(seat) => seat,
            None => return,
        };

        match request {
            wl_data_device::Request::StartDrag {
                source,
                origin,
                icon,
                serial,
            } => {
                let serial = Serial::from(serial);
                if let Some(pointer) = seat.get_pointer() {
                    if pointer.has_grab(serial) {
                        if let Some(ref icon) = icon {
                            if compositor::give_role(icon, DND_ICON_ROLE).is_err() {
                                resource.post_error(
                                    wl_data_device::Error::Role,
                                    "Given surface already has an other role",
                                );
                                return;
                            }
                        }
                        // The StartDrag is in response to a pointer implicit grab, all is good
                        handler.started(source.clone(), icon.clone(), seat.clone());
                        let start_data = pointer.grab_start_data().unwrap();
                        pointer.set_grab(
                            handler,
                            dnd_grab::DnDGrab::new(dh, start_data, source, origin, seat, icon),
                            serial,
                            Focus::Clear,
                        );
                        return;
                    }
                }
                debug!(serial = ?serial, client = ?client, "denying drag from client without implicit grab");
            }
            wl_data_device::Request::SetSelection { source, .. } => {
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
                let source = source.map(SelectionSourceProvider::DataDevice);

                handler.new_selection(
                    SelectionTarget::Clipboard,
                    source.clone().map(|provider| SelectionSource { provider }),
                    seat.clone(),
                );

                // The client has kbd focus, it can set the selection
                seat_data
                    .borrow_mut()
                    .set_clipboard_selection::<D>(dh, source.map(OfferReplySource::Client));
            }
            wl_data_device::Request::Release => seat
                .user_data()
                .get::<RefCell<SeatData<D::SelectionUserData>>>()
                .unwrap()
                .borrow_mut()
                .retain_devices(|ndd| match ndd {
                    SelectionDevice::DataDevice(ndd) => ndd != resource,
                    _ => true,
                }),

            _ => unreachable!(),
        }
    }
}
