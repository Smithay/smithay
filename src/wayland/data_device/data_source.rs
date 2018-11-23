use std::sync::Mutex;

use wayland_server::{
    protocol::{
        wl_data_device_manager::DndAction,
        wl_data_source::{Request, WlDataSource},
    },
    NewResource, Resource,
};

/// The metadata describing a data source
#[derive(Debug, Clone)]
pub struct SourceMetadata {
    /// The MIME types supported by this source
    pub mime_types: Vec<String>,
    /// The Drag'n'Drop actions supported by this source
    pub dnd_action: DndAction,
}

pub(crate) fn implement_data_source(src: NewResource<WlDataSource>) -> Resource<WlDataSource> {
    src.implement(
        |req, me| {
            let data: &Mutex<SourceMetadata> = me.user_data().unwrap();
            let mut guard = data.lock().unwrap();
            match req {
                Request::Offer { mime_type } => guard.mime_types.push(mime_type),
                Request::SetActions { dnd_actions } => {
                    guard.dnd_action = DndAction::from_bits_truncate(dnd_actions);
                }
                Request::Destroy => {}
            }
        },
        None::<fn(_)>,
        Mutex::new(SourceMetadata {
            mime_types: Vec::new(),
            dnd_action: DndAction::None,
        }),
    )
}

/// Access the metadata of a data source
pub fn with_source_metadata<T, F: FnOnce(&SourceMetadata) -> T>(
    source: &Resource<WlDataSource>,
    f: F,
) -> Result<T, ()> {
    match source.user_data::<Mutex<SourceMetadata>>() {
        Some(data) => Ok(f(&data.lock().unwrap())),
        None => Err(()),
    }
}
