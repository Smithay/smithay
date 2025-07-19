use std::cell::RefCell;
use std::sync::Arc;

use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_device_v1::{
    self, ZwlrDataControlDeviceV1,
};
use wayland_server::protocol::wl_seat::WlSeat;
use wayland_server::{Client, Dispatch, DisplayHandle, Resource};

use crate::input::Seat;
use crate::wayland::selection::device::SelectionDevice;
use crate::wayland::selection::offer::OfferReplySource;
use crate::wayland::selection::seat_data::SeatData;
use crate::wayland::selection::source::SelectionSourceProvider;
use crate::wayland::selection::{SelectionSource, SelectionTarget};

use super::{DataControlHandler, DataControlState};

#[allow(missing_debug_implementations)]
#[doc(hidden)]
pub struct DataControlDeviceUserData {
    pub(crate) primary_selection_filter: Arc<Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>>,
    pub(crate) wl_seat: WlSeat,
}

impl<D> Dispatch<ZwlrDataControlDeviceV1, DataControlDeviceUserData, D> for DataControlState
where
    D: Dispatch<ZwlrDataControlDeviceV1, DataControlDeviceUserData>,
    D: DataControlHandler,
    D: 'static,
{
    fn request(
        handler: &mut D,
        client: &Client,
        resource: &ZwlrDataControlDeviceV1,
        request: <ZwlrDataControlDeviceV1 as wayland_server::Resource>::Request,
        data: &DataControlDeviceUserData,
        dh: &DisplayHandle,
        _: &mut wayland_server::DataInit<'_, D>,
    ) {
        let seat = match Seat::<D>::from_resource(&data.wl_seat) {
            Some(seat) => seat,
            None => return,
        };

        match request {
            zwlr_data_control_device_v1::Request::SetSelection { source, .. } => {
                // Each source can only be used once.
                if let Some(source) = source.as_ref() {
                    if handler
                        .data_control_state()
                        .used_sources
                        .insert(source.clone(), data.wl_seat.clone())
                        .is_some()
                    {
                        resource.post_error(
                            zwlr_data_control_device_v1::Error::UsedSource,
                            "selection source can be used only once.",
                        );
                        return;
                    }
                }

                seat.user_data()
                    .insert_if_missing(|| RefCell::new(SeatData::<D::SelectionUserData>::new()));

                let source = source.map(SelectionSourceProvider::WlrDataControl);

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
            zwlr_data_control_device_v1::Request::SetPrimarySelection { source, .. } => {
                // When the primary selection is disabled, we should simply ignore the requests.
                if !(*data.primary_selection_filter)(client) {
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
                            zwlr_data_control_device_v1::Error::UsedSource,
                            "selection source can be used only once.",
                        );
                        return;
                    }
                }

                seat.user_data()
                    .insert_if_missing(|| RefCell::new(SeatData::<D::SelectionUserData>::new()));

                let source = source.map(SelectionSourceProvider::WlrDataControl);

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
            zwlr_data_control_device_v1::Request::Destroy => seat
                .user_data()
                .get::<RefCell<SeatData<D::SelectionUserData>>>()
                .unwrap()
                .borrow_mut()
                .retain_devices(|ndd| match ndd {
                    SelectionDevice::WlrDataControl(ndd) => ndd != resource,
                    _ => true,
                }),

            _ => unreachable!(),
        }
    }
}
