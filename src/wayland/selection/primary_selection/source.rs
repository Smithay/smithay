use std::sync::Mutex;

use wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_source_v1::{
    self as primary_source, ZwpPrimarySelectionSourceV1 as PrimarySource,
};
use wayland_server::{backend::ClientId, Dispatch, DisplayHandle, Resource};

use crate::utils::{alive_tracker::AliveTracker, IsAlive};

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
    alive_tracker: AliveTracker,
}

impl PrimarySourceUserData {
    pub(super) fn new() -> Self {
        Self {
            inner: Default::default(),
            alive_tracker: Default::default(),
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

    fn destroyed(_state: &mut D, _client: ClientId, _resource: &PrimarySource, data: &PrimarySourceUserData) {
        data.alive_tracker.destroy_notify();
    }
}

impl IsAlive for PrimarySource {
    fn alive(&self) -> bool {
        let data: &PrimarySourceUserData = self.data().unwrap();
        data.alive_tracker.alive()
    }
}
