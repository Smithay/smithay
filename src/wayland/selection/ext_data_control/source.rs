use std::sync::Mutex;

use wayland_server::backend::ClientId;
use wayland_server::{Dispatch, DisplayHandle, Resource};

use crate::utils::alive_tracker::AliveTracker;
use crate::utils::IsAlive;

use wayland_protocols::ext::data_control::v1::server::ext_data_control_source_v1::{
    self, ExtDataControlSourceV1,
};

use super::{DataControlHandler, DataControlState};

#[doc(hidden)]
#[derive(Default, Debug)]
pub struct ExtDataControlSourceUserData {
    pub(crate) inner: Mutex<SourceMetadata>,
    alive_tracker: AliveTracker,
}

impl ExtDataControlSourceUserData {
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

impl<D> Dispatch<ExtDataControlSourceV1, ExtDataControlSourceUserData, D> for DataControlState
where
    D: Dispatch<ExtDataControlSourceV1, ExtDataControlSourceUserData>,
    D: DataControlHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &ExtDataControlSourceV1,
        request: <ExtDataControlSourceV1 as wayland_server::Resource>::Request,
        data: &ExtDataControlSourceUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            ext_data_control_source_v1::Request::Offer { mime_type } => {
                let mut data = data.inner.lock().unwrap();
                data.mime_types.push(mime_type);
            }
            ext_data_control_source_v1::Request::Destroy => (),
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut D,
        _client: ClientId,
        _resource: &ExtDataControlSourceV1,
        data: &ExtDataControlSourceUserData,
    ) {
        data.alive_tracker.destroy_notify();
    }
}

impl IsAlive for ExtDataControlSourceV1 {
    #[inline]
    fn alive(&self) -> bool {
        let data: &ExtDataControlSourceUserData = self.data().unwrap();
        data.alive_tracker.alive()
    }
}
