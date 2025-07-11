use std::{cell::RefCell, sync::Mutex};

use wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_source_v1::{
    self as primary_source, ZwpPrimarySelectionSourceV1 as PrimarySource,
};
use wayland_server::{backend::ClientId, Dispatch, DisplayHandle};

use crate::{
    input::Seat,
    wayland::selection::{offer::OfferReplySource, seat_data::SeatData, source::SelectionSourceProvider},
};

use super::{PrimarySelectionHandler, PrimarySelectionState};

/// The metadata describing a data source
#[derive(Debug, Default, Clone)]
pub struct SourceMetadata {
    /// The MIME types supported by this source
    pub mime_types: Vec<String>,
}

#[doc(hidden)]
#[derive(Debug)]
pub struct PrimarySourceUserData {
    pub(crate) inner: Mutex<SourceMetadata>,
    display_handle: DisplayHandle,
}

impl PrimarySourceUserData {
    pub(super) fn new(display_handle: DisplayHandle) -> Self {
        Self {
            inner: Default::default(),
            display_handle,
        }
    }
}

impl<D> Dispatch<PrimarySource, PrimarySourceUserData, D> for PrimarySelectionState
where
    D: Dispatch<PrimarySource, PrimarySourceUserData>,
    D: PrimarySelectionHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _resource: &PrimarySource,
        request: primary_source::Request,
        data: &PrimarySourceUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let _primary_selection_state = state.primary_selection_state();
        let mut data = data.inner.lock().unwrap();

        match request {
            primary_source::Request::Offer { mime_type } => {
                data.mime_types.push(mime_type);
            }
            primary_source::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(state: &mut D, _client: ClientId, source: &PrimarySource, data: &PrimarySourceUserData) {
        // Remove the source from the used ones.
        let seat = match state
            .primary_selection_state()
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

        match seat_data.get_primary_selection() {
            Some(OfferReplySource::Client(SelectionSourceProvider::Primary(set_source)))
                if set_source == source =>
            {
                seat_data.set_primary_selection::<D>(&data.display_handle, None)
            }
            _ => (),
        }
    }
}
