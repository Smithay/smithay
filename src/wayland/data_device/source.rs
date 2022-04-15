use slog::error;
use std::sync::Mutex;

use wayland_server::{
    protocol::wl_data_source::{self},
    protocol::{wl_data_device_manager::DndAction, wl_data_source::WlDataSource},
    DelegateDispatch, DelegateDispatchBase, Dispatch, Resource,
};

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

#[derive(Debug)]
pub struct DataSourceUserData {
    inner: Mutex<SourceMetadata>,
}

impl DataSourceUserData {
    pub(super) fn new() -> Self {
        Self {
            inner: Default::default(),
        }
    }
}

impl DelegateDispatchBase<WlDataSource> for DataDeviceState {
    type UserData = DataSourceUserData;
}

impl<D> DelegateDispatch<WlDataSource, D> for DataDeviceState
where
    D: Dispatch<WlDataSource, UserData = DataSourceUserData>,
    D: DataDeviceHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WlDataSource,
        request: wl_data_source::Request,
        data: &Self::UserData,
        _dhandle: &mut wayland_server::DisplayHandle<'_>,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let data_device_state = state.data_device_state();
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
                    error!(&data_device_state.log, "Unknown dnd_action: {:?}", action);
                }
            },
            wl_data_source::Request::Destroy => {}
            _ => unreachable!(),
        }
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
