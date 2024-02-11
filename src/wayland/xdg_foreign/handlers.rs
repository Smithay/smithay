use std::collections::HashSet;

use wayland_protocols::xdg::foreign::zv2::server::{
    zxdg_exported_v2::{self, ZxdgExportedV2},
    zxdg_exporter_v2::{self, ZxdgExporterV2},
    zxdg_imported_v2::{self, ZxdgImportedV2},
    zxdg_importer_v2::{self, ZxdgImporterV2},
};
use wayland_server::{backend::ClientId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};

use crate::wayland::{compositor, shell::xdg::XdgToplevelSurfaceData};

use super::{
    ExportedState, XdgExportedUserData, XdgForeignHandle, XdgForeignHandler, XdgForeignState,
    XdgImportedUserData,
};

//
// Export
//

impl<D: XdgForeignHandler> GlobalDispatch<ZxdgExporterV2, (), D> for XdgForeignState {
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZxdgExporterV2>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init_delegated::<_, _, Self>(resource, ());
    }
}

impl<D: XdgForeignHandler> Dispatch<ZxdgExporterV2, (), D> for XdgForeignState {
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &ZxdgExporterV2,
        request: zxdg_exporter_v2::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zxdg_exporter_v2::Request::ExportToplevel { id, surface } => {
                let handle = XdgForeignHandle::new();
                let exported = data_init.init_delegated::<_, _, Self>(
                    id,
                    XdgExportedUserData {
                        handle: handle.clone(),
                    },
                );
                exported.handle(handle.as_str().to_owned());

                state.xdg_foreign_state().exported.insert(
                    handle,
                    ExportedState {
                        exported_surface: surface,
                        requested_parent: None,
                        imported_by: HashSet::new(),
                    },
                );
            }
            zxdg_exporter_v2::Request::Destroy => {}
            _ => {}
        }
    }
}

impl<D: XdgForeignHandler> Dispatch<ZxdgExportedV2, XdgExportedUserData, D> for XdgForeignState {
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &ZxdgExportedV2,
        _request: zxdg_exported_v2::Request,
        _data: &XdgExportedUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(state: &mut D, _client: ClientId, _resource: &ZxdgExportedV2, data: &XdgExportedUserData) {
        // Revoke the previously exported surface.
        // This invalidates any relationship the importer may have set up using the xdg_imported created given the handle sent via xdg_exported.handle.
        if let Some(mut state) = state.xdg_foreign_state().exported.remove(&data.handle) {
            invalidate_all_relationships(&mut state);
        }
    }
}

//
// Import
//

impl<D: XdgForeignHandler> GlobalDispatch<ZxdgImporterV2, (), D> for XdgForeignState {
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZxdgImporterV2>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init_delegated::<_, _, Self>(resource, ());
    }
}

impl<D: XdgForeignHandler> Dispatch<ZxdgImporterV2, (), D> for XdgForeignState {
    fn request(
        state: &mut D,
        _client: &Client,
        _resource: &ZxdgImporterV2,
        request: zxdg_importer_v2::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zxdg_importer_v2::Request::ImportToplevel { id, handle } => {
                let exported = state
                    .xdg_foreign_state()
                    .exported
                    .iter_mut()
                    .find(|(key, _)| key.as_str() == handle.as_str());

                let imported = data_init.init_delegated::<_, _, Self>(
                    id,
                    XdgImportedUserData {
                        handle: XdgForeignHandle(handle),
                    },
                );

                match exported {
                    Some((_, state)) => {
                        state.imported_by.insert(imported);
                    }
                    None => {
                        imported.destroyed();
                    }
                }
            }
            zxdg_importer_v2::Request::Destroy => {}
            _ => {}
        }
    }
}

impl<D: XdgForeignHandler> Dispatch<ZxdgImportedV2, XdgImportedUserData, D> for XdgForeignState {
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &ZxdgImportedV2,
        request: zxdg_imported_v2::Request,
        data: &XdgImportedUserData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zxdg_imported_v2::Request::SetParentOf { surface } => {
                if let Some((_, state)) = state
                    .xdg_foreign_state()
                    .exported
                    .iter_mut()
                    .find(|(key, _)| key.as_str() == data.handle.as_str())
                {
                    compositor::with_states(&state.exported_surface, |states| {
                        if let Some(data) = states.data_map.get::<XdgToplevelSurfaceData>() {
                            data.lock().unwrap().parent = Some(surface.clone());
                        }
                    });

                    state.requested_parent = Some((surface, resource.clone()));
                }
            }
            zxdg_imported_v2::Request::Destroy => {}
            _ => {}
        }
    }

    fn destroyed(state: &mut D, _client: ClientId, resource: &ZxdgImportedV2, data: &XdgImportedUserData) {
        if let Some((_, state)) = state
            .xdg_foreign_state()
            .exported
            .iter_mut()
            .find(|(key, _)| key.as_str() == data.handle.as_str())
        {
            state.imported_by.remove(resource);
            invalidate_relationship_for(state, Some(resource));
        }
    }
}

fn invalidate_all_relationships(state: &mut ExportedState) {
    invalidate_relationship_for(state, None);
}

fn invalidate_relationship_for(state: &mut ExportedState, invalidate_for: Option<&ZxdgImportedV2>) {
    let Some((requested_parent, requested_by)) = state.requested_parent.as_ref() else {
        return;
    };

    if let Some(invalidate_for) = invalidate_for {
        if invalidate_for != requested_by {
            return;
        }
    }

    compositor::with_states(&state.exported_surface, |states| {
        let Some(data) = states.data_map.get::<XdgToplevelSurfaceData>() else {
            return;
        };

        let data = &mut *data.lock().unwrap();
        if data.parent.as_ref() == Some(requested_parent) {
            data.parent = None;
        }
    });

    state.requested_parent = None;
}
