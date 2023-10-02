use std::sync::Mutex;
use tracing::error;

use wayland_server::{
    backend::ClientId,
    protocol::wl_data_source::{self},
    protocol::{wl_data_device_manager::DndAction, wl_data_source::WlDataSource},
    Dispatch, DisplayHandle, Resource,
};

use crate::utils::{alive_tracker::AliveTracker, IsAlive};

use super::{DataDeviceHandler, DataDeviceState};

/// The metadata describing a data source
#[derive(Debug, Clone)]
pub struct SourceMetadata {
    /// The MIME types supported by this source
    pub mime_types: Vec<String>,
    /// The Drag'n'Drop actions supported by this source
    pub dnd_action: DndAction,
}

impl Default for SourceMetadata {
    fn default() -> Self {
        Self {
            mime_types: Vec::new(),
            dnd_action: DndAction::None,
        }
    }
}

#[doc(hidden)]
#[derive(Debug)]
pub struct DataSourceUserData {
    pub(crate) inner: Mutex<SourceMetadata>,
    alive_tracker: AliveTracker,
}

impl DataSourceUserData {
    pub(super) fn new() -> Self {
        Self {
            inner: Default::default(),
            alive_tracker: Default::default(),
        }
    }
}

impl<D> Dispatch<WlDataSource, DataSourceUserData, D> for DataDeviceState
where
    D: Dispatch<WlDataSource, DataSourceUserData>,
    D: DataDeviceHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlDataSource,
        request: wl_data_source::Request,
        data: &DataSourceUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let mut data = data.inner.lock().unwrap();

        match request {
            wl_data_source::Request::Offer { mime_type } => {
                data.mime_types.push(mime_type);
            }
            wl_data_source::Request::SetActions { dnd_actions } => match dnd_actions {
                wayland_server::WEnum::Value(dnd_actions) => {
                    data.dnd_action = dnd_actions;
                }
                wayland_server::WEnum::Unknown(action) => {
                    error!("Unknown dnd_action: {:?}", action);
                }
            },
            wl_data_source::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(_state: &mut D, _client: ClientId, _resource: &WlDataSource, data: &DataSourceUserData) {
        data.alive_tracker.destroy_notify();
    }
}

impl IsAlive for WlDataSource {
    fn alive(&self) -> bool {
        let data: &DataSourceUserData = self.data().unwrap();
        data.alive_tracker.alive()
    }
}

/// Access the metadata of a data source
pub fn with_source_metadata<T, F: FnOnce(&SourceMetadata) -> T>(
    source: &WlDataSource,
    f: F,
) -> Result<T, crate::utils::UnmanagedResource> {
    match source.data::<DataSourceUserData>() {
        Some(data) => Ok(f(&data.inner.lock().unwrap())),
        None => Err(crate::utils::UnmanagedResource),
    }
}
