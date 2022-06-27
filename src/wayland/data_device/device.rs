use std::cell::RefCell;

use slog::debug;
use wayland_server::{
    protocol::{
        wl_data_device::{self, WlDataDevice},
        wl_seat::WlSeat,
    },
    Client, DataInit, DelegateDispatch, Dispatch, DisplayHandle, Resource,
};

use crate::wayland::{
    compositor,
    data_device::seat_data::{SeatData, Selection},
    seat::{Focus, Seat},
    Serial,
};

use super::{dnd_grab, DataDeviceHandler, DataDeviceState};

/// WlSurface role of drag and drop icon
pub const DND_ICON_ROLE: &str = "dnd_icon";

#[doc(hidden)]
#[derive(Debug)]
pub struct DataDeviceUserData {
    pub(crate) wl_seat: WlSeat,
}

impl<D> DelegateDispatch<WlDataDevice, DataDeviceUserData, D> for DataDeviceState
where
    D: Dispatch<WlDataDevice, DataDeviceUserData>,
    D: DataDeviceHandler,
    D: 'static,
{
    fn request(
        handler: &mut D,
        _client: &Client,
        resource: &WlDataDevice,
        request: wl_data_device::Request,
        data: &DataDeviceUserData,
        dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        let data_device_state = handler.data_device_state();

        if let Some(seat) = Seat::<D>::from_resource(&data.wl_seat) {
            match request {
                wl_data_device::Request::StartDrag {
                    source,
                    origin,
                    icon,
                    serial,
                } => {
                    /* TODO: handle the icon */
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
                                dnd_grab::DnDGrab::new(start_data, source, origin, seat, icon),
                                serial,
                                Focus::Clear,
                            );
                            return;
                        }
                    }
                    debug!(
                        &data_device_state.log,
                        "denying drag from client without implicit grab"
                    );
                }
                wl_data_device::Request::SetSelection { source, .. } => {
                    if let Some(keyboard) = seat.get_keyboard() {
                        if keyboard.client_of_object_has_focus(&resource.id()) {
                            let seat_data = seat.user_data().get::<RefCell<SeatData>>().unwrap();

                            handler.new_selection(dh, source.clone());
                            // The client has kbd focus, it can set the selection
                            seat_data.borrow_mut().set_selection::<D>(
                                dh,
                                source.map(Selection::Client).unwrap_or(Selection::Empty),
                            );
                            return;
                        }
                    }
                    debug!(
                        &data_device_state.log,
                        "denying setting selection by a non-focused client"
                    );
                }
                wl_data_device::Request::Release => {
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
