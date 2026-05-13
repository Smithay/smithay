//! `wl_fixes` protocol

use tracing::error;
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, backend::GlobalId,
    protocol::wl_fixes,
};

use crate::wayland::GlobalData;

/// Delegate type for handling wl fixes requests.
#[derive(Debug, Clone)]
pub struct FixesState {
    global: GlobalId,
}

impl FixesState {
    /// Creates a new delegate type for handling [`wl_fixes::WlFixes`] events.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: 'static,
    {
        let global = display.create_global::<D, wl_fixes::WlFixes, _>(1, GlobalData);
        Self { global }
    }

    /// Returns the wl fixes global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<wl_fixes::WlFixes, D> for GlobalData
where
    D: 'static,
{
    fn bind(
        &self,
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<wl_fixes::WlFixes>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }
}

impl<D> Dispatch<wl_fixes::WlFixes, D> for GlobalData
where
    D: 'static,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &Client,
        _resource: &wl_fixes::WlFixes,
        request: wl_fixes::Request,
        dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wl_fixes::Request::DestroyRegistry { registry } => {
                if let Err(err) = dh.backend_handle().destroy_object::<D>(&registry.id()) {
                    error!(?err, "failed to destroy registry");
                }
            }
            wl_fixes::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}
