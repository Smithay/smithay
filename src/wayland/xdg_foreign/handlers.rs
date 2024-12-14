use std::collections::HashSet;

use wayland_protocols::xdg::foreign::zv2::server::{
    zxdg_exported_v2::{self, ZxdgExportedV2},
    zxdg_exporter_v2::{self, ZxdgExporterV2},
    zxdg_imported_v2::{self, ZxdgImportedV2},
    zxdg_importer_v2::{self, ZxdgImporterV2},
};
use wayland_server::{
    backend::ClientId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::wayland::{
    compositor,
    shell::{
        is_valid_parent,
        xdg::{XdgShellHandler, XdgToplevelSurfaceData},
    },
};

use super::{
    ExportedState, XdgExportedUserData, XdgForeignHandle, XdgForeignHandler, XdgForeignState,
    XdgImportedUserData,
};

//
// Export
//

impl<D> GlobalDispatch<ZxdgExporterV2, (), D> for XdgForeignState
where
    D: Dispatch<ZxdgExporterV2, ()>,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZxdgExporterV2>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<ZxdgExporterV2, (), D> for XdgForeignState
where
    D: Dispatch<ZxdgExportedV2, XdgExportedUserData>,
    D: XdgForeignHandler,
{
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
                let exported = data_init.init(
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
                        requested_child: None,
                        imported_by: HashSet::new(),
                    },
                );
            }
            zxdg_exporter_v2::Request::Destroy => {}
            _ => {}
        }
    }
}

impl<D> Dispatch<ZxdgExportedV2, XdgExportedUserData, D> for XdgForeignState
where
    D: XdgForeignHandler + XdgShellHandler,
{
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
        invalidate_all_relationships(state, &data.handle);
        state.xdg_foreign_state().exported.remove(&data.handle);
    }
}

//
// Import
//

impl<D> GlobalDispatch<ZxdgImporterV2, (), D> for XdgForeignState
where
    D: Dispatch<ZxdgImporterV2, ()>,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZxdgImporterV2>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D: XdgForeignHandler> Dispatch<ZxdgImporterV2, (), D> for XdgForeignState
where
    D: Dispatch<ZxdgImportedV2, XdgImportedUserData>,
{
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

                let imported = data_init.init(
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

impl<D> Dispatch<ZxdgImportedV2, XdgImportedUserData, D> for XdgForeignState
where
    D: XdgForeignHandler + XdgShellHandler,
{
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
            zxdg_imported_v2::Request::SetParentOf { surface: child } => {
                if let Some((_, exported_state)) = state
                    .xdg_foreign_state()
                    .exported
                    .iter_mut()
                    .find(|(key, _)| key.as_str() == data.handle.as_str())
                {
                    let parent = &exported_state.exported_surface;

                    let mut invalid = false;
                    let mut changed = false;
                    compositor::with_states(&child, |states| {
                        if let Some(data) = states.data_map.get::<XdgToplevelSurfaceData>() {
                            if is_valid_parent(&child, parent) {
                                let mut role = data.lock().unwrap();
                                changed = role.parent.as_ref() != Some(parent);
                                role.parent = Some(parent.clone());
                            } else {
                                invalid = true;
                            }
                        }
                    });

                    if invalid {
                        resource.post_error(
                            zxdg_imported_v2::Error::InvalidSurface,
                            "invalid parent relationship",
                        );
                        return;
                    }

                    exported_state.requested_child = Some((child.clone(), resource.clone()));

                    if changed {
                        if let Some(toplevel) = state
                            .xdg_shell_state()
                            .toplevel_surfaces()
                            .iter()
                            .find(|toplevel| *toplevel.wl_surface() == child)
                            .cloned()
                        {
                            XdgShellHandler::parent_changed(state, toplevel);
                        }
                    }
                }
            }
            zxdg_imported_v2::Request::Destroy => {}
            _ => {}
        }
    }

    fn destroyed(state: &mut D, _client: ClientId, resource: &ZxdgImportedV2, data: &XdgImportedUserData) {
        if let Some((_, exported_state)) = state
            .xdg_foreign_state()
            .exported
            .iter_mut()
            .find(|(key, _)| key.as_str() == data.handle.as_str())
        {
            exported_state.imported_by.remove(resource);
        }

        invalidate_relationship_for(state, &data.handle, Some(resource));
    }
}

fn invalidate_all_relationships<D>(state: &mut D, handle: &XdgForeignHandle)
where
    D: XdgForeignHandler + XdgShellHandler,
{
    invalidate_relationship_for(state, handle, None);
}

fn invalidate_relationship_for<D>(
    state: &mut D,
    handle: &XdgForeignHandle,
    invalidate_for: Option<&ZxdgImportedV2>,
) where
    D: XdgForeignHandler + XdgShellHandler,
{
    let Some((_, exported_state)) = state
        .xdg_foreign_state()
        .exported
        .iter_mut()
        .find(|(key, _)| key.as_str() == handle.as_str())
    else {
        return;
    };

    let Some((requested_child, requested_by)) = exported_state.requested_child.as_ref() else {
        return;
    };

    if let Some(invalidate_for) = invalidate_for {
        if invalidate_for != requested_by {
            return;
        }
    }

    let mut changed = false;
    compositor::with_states(requested_child, |states| {
        let Some(data) = states.data_map.get::<XdgToplevelSurfaceData>() else {
            return;
        };

        let data = &mut *data.lock().unwrap();
        if data.parent.as_ref() == Some(&exported_state.exported_surface) {
            data.parent = None;
            changed = true;
        }
    });

    let requested_child = requested_child.clone();
    exported_state.requested_child = None;

    if changed {
        if let Some(toplevel) = state
            .xdg_shell_state()
            .toplevel_surfaces()
            .iter()
            .find(|toplevel| *toplevel.wl_surface() == requested_child)
            .cloned()
        {
            XdgShellHandler::parent_changed(state, toplevel);
        }
    }
}
