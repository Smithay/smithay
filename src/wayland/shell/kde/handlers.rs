//! Handlers for KDE decoration events.
use tracing::trace;

use wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::{
    OrgKdeKwinServerDecoration, Request,
};
use wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::{
    OrgKdeKwinServerDecorationManager, Request as ManagerRequest,
};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use crate::wayland::GlobalData;
use crate::wayland::shell::kde::decoration::KdeDecorationHandler;

use super::decoration::{KdeDecorationManagerGlobalData, KwinServerDecorationData};

impl<D> GlobalDispatch<OrgKdeKwinServerDecorationManager, D> for KdeDecorationManagerGlobalData
where
    D: KdeDecorationHandler + 'static,
{
    fn bind(
        &self,
        state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<OrgKdeKwinServerDecorationManager>,
        data_init: &mut DataInit<'_, D>,
    ) {
        let kde_decoration_manager = data_init.init(resource, GlobalData);

        // Set default decoration mode.
        let default_mode = state.kde_decoration_state().default_mode;
        kde_decoration_manager.default_mode(default_mode);

        trace!("Bound decoration manager global");
    }

    fn can_view(&self, client: &Client) -> bool {
        (self.filter)(client)
    }
}

impl<D> Dispatch<OrgKdeKwinServerDecorationManager, D> for GlobalData
where
    D: KdeDecorationHandler + 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        _kde_decoration_manager: &OrgKdeKwinServerDecorationManager,
        request: ManagerRequest,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let (id, surface) = match request {
            ManagerRequest::Create { id, surface } => (id, surface),
            _ => unreachable!(),
        };

        let kde_decoration = data_init.init(id, KwinServerDecorationData(surface));

        let surface = &kde_decoration.data::<KwinServerDecorationData>().unwrap().0;
        state.new_decoration(surface, &kde_decoration);

        trace!(surface = ?surface, "Created decoration object for surface");
    }
}

impl<D> Dispatch<OrgKdeKwinServerDecoration, D> for KwinServerDecorationData
where
    D: KdeDecorationHandler + 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        kde_decoration: &OrgKdeKwinServerDecoration,
        request: Request,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        let surface = &self.0;

        trace!(
            surface = ?surface,
            request = ?request,
            "Decoration request for surface"
        );

        match request {
            Request::RequestMode { mode } => state.request_mode(surface, kde_decoration, mode),
            Request::Release => state.release(kde_decoration, surface),
            _ => unreachable!(),
        }
    }
}
