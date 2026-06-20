use std::cell::RefCell;
use std::sync::Mutex;

use wayland_server::DisplayHandle;
use wayland_server::backend::ClientId;

use crate::input::Seat;
use crate::wayland::Dispatch2;
use crate::wayland::selection::SelectionTarget;
use crate::wayland::selection::offer::OfferReplySource;
use crate::wayland::selection::seat_data::SeatData;
use crate::wayland::selection::source::SelectionSourceProvider;

use wayland_protocols::ext::data_control::v1::server::ext_data_control_source_v1::{
    self, ExtDataControlSourceV1,
};

use super::DataControlHandler;

#[doc(hidden)]
#[derive(Debug)]
pub struct ExtDataControlSourceUserData {
    pub(crate) inner: Mutex<SourceMetadata>,
    display_handle: DisplayHandle,
}

impl ExtDataControlSourceUserData {
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

impl<D> Dispatch2<ExtDataControlSourceV1, D> for ExtDataControlSourceUserData
where
    D: DataControlHandler,
    D: 'static,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &ExtDataControlSourceV1,
        request: <ExtDataControlSourceV1 as wayland_server::Resource>::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            ext_data_control_source_v1::Request::Offer { mime_type } => {
                let mut data = self.inner.lock().unwrap();
                data.mime_types.push(mime_type);
            }
            ext_data_control_source_v1::Request::Destroy => (),
            _ => unreachable!(),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, source: &ExtDataControlSourceV1) {
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
                Some(OfferReplySource::Client(SelectionSourceProvider::ExtDataControl(set_source)))
                    if set_source == source =>
                {
                    match target {
                        SelectionTarget::Primary => {
                            seat_data.set_primary_selection::<D>(&self.display_handle, None)
                        }
                        SelectionTarget::Clipboard => {
                            seat_data.set_clipboard_selection::<D>(&self.display_handle, None)
                        }
                    }
                }
                _ => (),
            };
        }
    }
}
