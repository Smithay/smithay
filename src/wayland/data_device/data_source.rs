use std::cell::RefCell;

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

pub(crate) fn implement_data_source(src: NewResource<WlDataSource>) -> WlDataSource {
    src.implement_closure(
        |req, me| {
            let data: &RefCell<SourceMetadata> = me.as_ref().user_data().unwrap();
            let mut guard = data.borrow_mut();
            match req {
                Request::Offer { mime_type } => guard.mime_types.push(mime_type),
                Request::SetActions { dnd_actions } => {
                    guard.dnd_action = DndAction::from_bits_truncate(dnd_actions);
                }
                Request::Destroy => {}
                _ => unreachable!(),
            }
        },
        None::<fn(_)>,
        RefCell::new(SourceMetadata {
            mime_types: Vec::new(),
            dnd_action: DndAction::None,
        }),
    )
}

/// Access the metadata of a data source
pub fn with_source_metadata<T, F: FnOnce(&SourceMetadata) -> T>(
    source: &WlDataSource,
    f: F,
) -> Result<T, ()> {
    match source.as_ref().user_data::<RefCell<SourceMetadata>>() {
        Some(data) => Ok(f(&data.borrow())),
        None => Err(()),
    }
}
