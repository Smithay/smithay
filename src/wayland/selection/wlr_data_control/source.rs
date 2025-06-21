use std::cell::RefCell;
use std::sync::Mutex;

use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_source_v1::{
    self, ZwlrDataControlSourceV1,
};
use wayland_server::backend::ClientId;
use wayland_server::{Dispatch, DisplayHandle};

use crate::input::Seat;
use crate::wayland::selection::offer::OfferReplySource;
use crate::wayland::selection::seat_data::SeatData;
use crate::wayland::selection::source::SelectionSourceProvider;
use crate::wayland::selection::SelectionTarget;

use super::{DataControlHandler, DataControlState};

#[doc(hidden)]
#[derive(Debug)]
pub struct DataControlSourceUserData {
    pub(crate) inner: Mutex<SourceMetadata>,
    display_handle: DisplayHandle,
}

impl DataControlSourceUserData {
    pub(crate) fn new(display_handle: DisplayHandle) -> Self {
        Self {
            inner: Default::default(),
            display_handle,
        }
    }
}

/// The metadata describing a data source
#[derive(Debug, Default, Clone)]
pub struct SourceMetadata {
    /// The MIME types supported by this source
    pub mime_types: Vec<String>,
}

impl<D> Dispatch<ZwlrDataControlSourceV1, DataControlSourceUserData, D> for DataControlState
where
    D: Dispatch<ZwlrDataControlSourceV1, DataControlSourceUserData>,
    D: DataControlHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &ZwlrDataControlSourceV1,
        request: <ZwlrDataControlSourceV1 as wayland_server::Resource>::Request,
        data: &DataControlSourceUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwlr_data_control_source_v1::Request::Offer { mime_type } => {
                let mut data = data.inner.lock().unwrap();
                data.mime_types.push(mime_type);
            }
            zwlr_data_control_source_v1::Request::Destroy => (),
            _ => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        source: &ZwlrDataControlSourceV1,
        data: &DataControlSourceUserData,
    ) {
        // Remove the source from the used ones.
        let seat = match state
            .data_control_state()
            .used_sources
            .remove(source)
            .as_ref()
            .and_then(Seat::<D>::from_resource)
        {
            Some(seat) => seat,
            None => return,
        };

        let mut seat_data = seat
            .user_data()
            .get::<RefCell<SeatData<D::SelectionUserData>>>()
            .unwrap()
            .borrow_mut();

        for target in [SelectionTarget::Primary, SelectionTarget::Clipboard] {
            let selection = match target {
                SelectionTarget::Primary => seat_data.get_primary_selection(),
                SelectionTarget::Clipboard => seat_data.get_clipboard_selection(),
            };

            match selection {
                Some(OfferReplySource::Client(SelectionSourceProvider::WlrDataControl(set_source)))
                    if set_source == source =>
                {
                    match target {
                        SelectionTarget::Primary => {
                            seat_data.set_primary_selection::<D>(&data.display_handle, None)
                        }
                        SelectionTarget::Clipboard => {
                            seat_data.set_clipboard_selection::<D>(&data.display_handle, None)
                        }
                    }
                }
                _ => (),
            };
        }
    }
}
