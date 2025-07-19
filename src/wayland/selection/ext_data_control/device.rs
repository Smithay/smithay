use std::cell::RefCell;

use wayland_protocols::ext::data_control::v1::server::ext_data_control_device_v1::{
    self, ExtDataControlDeviceV1,
};
use wayland_server::protocol::wl_seat::WlSeat;
use wayland_server::Resource;
use wayland_server::{Client, Dispatch, DisplayHandle};

use crate::input::Seat;
use crate::wayland::selection::device::SelectionDevice;
use crate::wayland::selection::offer::OfferReplySource;
use crate::wayland::selection::seat_data::SeatData;
use crate::wayland::selection::source::SelectionSourceProvider;
use crate::wayland::selection::{SelectionSource, SelectionTarget};

use super::{DataControlHandler, DataControlState};

#[doc(hidden)]
#[derive(Debug)]
pub struct ExtDataControlDeviceUserData {
    pub(crate) primary: bool,
    pub(crate) wl_seat: WlSeat,
}

impl<D> Dispatch<ExtDataControlDeviceV1, ExtDataControlDeviceUserData, D> for DataControlState
where
    D: Dispatch<ExtDataControlDeviceV1, ExtDataControlDeviceUserData>,
    D: DataControlHandler,
    D: 'static,
{
    fn request(
        handler: &mut D,
        _client: &Client,
        resource: &ExtDataControlDeviceV1,
        request: <ExtDataControlDeviceV1 as wayland_server::Resource>::Request,
        data: &ExtDataControlDeviceUserData,
        dh: &DisplayHandle,
        _: &mut wayland_server::DataInit<'_, D>,
    ) {
        let seat = match Seat::<D>::from_resource(&data.wl_seat) {
            Some(seat) => seat,
            None => return,
        };

        match request {
            ext_data_control_device_v1::Request::SetSelection { source, .. } => {
                // Each source can only be used once.
                if let Some(source) = source.as_ref() {
                    if handler
                        .data_control_state()
                        .used_sources
                        .insert(source.clone(), data.wl_seat.clone())
                        .is_some()
                    {
                        resource.post_error(
                            ext_data_control_device_v1::Error::UsedSource,
                            "selection source can be used only once.",
                        );
                        return;
                    }
                }

                seat.user_data()
                    .insert_if_missing(|| RefCell::new(SeatData::<D::SelectionUserData>::new()));

                let source = source.map(SelectionSourceProvider::ExtDataControl);

                handler.new_selection(
                    SelectionTarget::Clipboard,
                    source.clone().map(|provider| SelectionSource { provider }),
                    seat.clone(),
                );

                seat.user_data()
                    .get::<RefCell<SeatData<D::SelectionUserData>>>()
                    .unwrap()
                    .borrow_mut()
                    .set_clipboard_selection::<D>(dh, source.map(OfferReplySource::Client));
            }
            ext_data_control_device_v1::Request::SetPrimarySelection { source, .. } => {
                // When the primary selection is disabled, we should simply ignore the requests.
                if !data.primary {
                    return;
                }

                // Each source can only be used once.
                if let Some(source) = source.as_ref() {
                    if handler
                        .data_control_state()
                        .used_sources
                        .insert(source.clone(), data.wl_seat.clone())
                        .is_some()
                    {
                        resource.post_error(
                            ext_data_control_device_v1::Error::UsedSource,
                            "selection source can be used only once.",
                        );
                        return;
                    }
                }

                seat.user_data()
                    .insert_if_missing(|| RefCell::new(SeatData::<D::SelectionUserData>::new()));

                let source = source.map(SelectionSourceProvider::ExtDataControl);

                handler.new_selection(
                    SelectionTarget::Primary,
                    source.clone().map(|provider| SelectionSource { provider }),
                    seat.clone(),
                );

                seat.user_data()
                    .get::<RefCell<SeatData<D::SelectionUserData>>>()
                    .unwrap()
                    .borrow_mut()
                    .set_primary_selection::<D>(dh, source.map(OfferReplySource::Client));
            }
            ext_data_control_device_v1::Request::Destroy => seat
                .user_data()
                .get::<RefCell<SeatData<D::SelectionUserData>>>()
                .unwrap()
                .borrow_mut()
                .retain_devices(|ndd| match ndd {
                    SelectionDevice::ExtDataControl(ndd) => ndd != resource,
                    _ => true,
                }),

            _ => unreachable!(),
        }
    }
}
