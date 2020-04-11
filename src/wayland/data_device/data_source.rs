use std::{cell::RefCell, ops::Deref as _};

use wayland_server::{
    protocol::{
        wl_data_device_manager::DndAction,
        wl_data_source::{Request, WlDataSource},
    },
    Main,
};

/// The metadata describing a data source
#[derive(Debug, Clone)]
pub struct SourceMetadata {
    /// The MIME types supported by this source
    pub mime_types: Vec<String>,
    /// The Drag'n'Drop actions supported by this source
    pub dnd_action: DndAction,
}

pub(crate) fn implement_data_source(src: Main<WlDataSource>) -> WlDataSource {
    src.quick_assign(|me, req, _| {
        let data: &RefCell<SourceMetadata> = me.as_ref().user_data().get().unwrap();
        let mut guard = data.borrow_mut();
        match req {
            Request::Offer { mime_type } => guard.mime_types.push(mime_type),
            Request::SetActions { dnd_actions } => {
                guard.dnd_action = DndAction::from_bits_truncate(dnd_actions);
            }
            Request::Destroy => {}
            _ => unreachable!(),
        }
    });
    src.as_ref().user_data().set(|| {
        RefCell::new(SourceMetadata {
            mime_types: Vec::new(),
            dnd_action: DndAction::None,
        })
    });

    src.deref().clone()
}

/// Access the metadata of a data source
pub fn with_source_metadata<T, F: FnOnce(&SourceMetadata) -> T>(
    source: &WlDataSource,
    f: F,
) -> Result<T, ()> {
    match source.as_ref().user_data().get::<RefCell<SourceMetadata>>() {
        Some(data) => Ok(f(&data.borrow())),
        None => Err(()),
    }
}
