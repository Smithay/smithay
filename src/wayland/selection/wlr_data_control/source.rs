use std::sync::Mutex;

use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_source_v1::{
    self, ZwlrDataControlSourceV1,
};
use wayland_server::backend::ClientId;
use wayland_server::{Dispatch, DisplayHandle, Resource};

use crate::utils::alive_tracker::AliveTracker;
use crate::utils::IsAlive;

use super::{DataControlHandler, DataControlState};

#[doc(hidden)]
#[derive(Default, Debug)]
pub struct DataControlSourceUserData {
    pub(crate) inner: Mutex<SourceMetadata>,
    alive_tracker: AliveTracker,
}

impl DataControlSourceUserData {
    pub(crate) fn new() -> Self {
        Self::default()
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
        _state: &mut D,
        _client: ClientId,
        _resource: &ZwlrDataControlSourceV1,
        data: &DataControlSourceUserData,
    ) {
        data.alive_tracker.destroy_notify();
    }
}

impl IsAlive for ZwlrDataControlSourceV1 {
    fn alive(&self) -> bool {
        let data: &DataControlSourceUserData = self.data().unwrap();
        data.alive_tracker.alive()
    }
}
