//! Handlers for KDE decoration events.

use wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::{
    OrgKdeKwinServerDecoration, Request,
};
use wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::{
    OrgKdeKwinServerDecorationManager, Request as ManagerRequest,
};
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use crate::wayland::shell::kde::decoration::{KdeDecorationHandler, KdeDecorationState};

impl<D> GlobalDispatch<OrgKdeKwinServerDecorationManager, (), D> for KdeDecorationState
where
    D: GlobalDispatch<OrgKdeKwinServerDecorationManager, ()>
        + Dispatch<OrgKdeKwinServerDecorationManager, ()>
        + Dispatch<OrgKdeKwinServerDecoration, WlSurface>
        + KdeDecorationHandler
        + 'static,
{
    fn bind(
        state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<OrgKdeKwinServerDecorationManager>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        let kde_decoration_manager = data_init.init(resource, ());

        // Set default decoration mode.
        let default_mode = state.kde_decoration_state().default_mode;
        kde_decoration_manager.default_mode(default_mode);

        let logger = &state.kde_decoration_state().logger;
        slog::trace!(logger, "Bound decoration manager global");
    }
}

impl<D> Dispatch<OrgKdeKwinServerDecorationManager, (), D> for KdeDecorationState
where
    D: Dispatch<OrgKdeKwinServerDecorationManager, ()>
        + Dispatch<OrgKdeKwinServerDecorationManager, ()>
        + Dispatch<OrgKdeKwinServerDecoration, WlSurface>
        + KdeDecorationHandler
        + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        _kde_decoration_manager: &OrgKdeKwinServerDecorationManager,
        request: ManagerRequest,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let (id, surface) = match request {
            ManagerRequest::Create { id, surface } => (id, surface),
            _ => unreachable!(),
        };

        let kde_decoration = data_init.init(id, surface);

        let surface = kde_decoration.data().unwrap();
        state.new_decoration(surface, &kde_decoration);

        let logger = &state.kde_decoration_state().logger;
        slog::trace!(logger, "Created decoration object for surface {:?}", surface);
    }
}

impl<D> Dispatch<OrgKdeKwinServerDecoration, WlSurface, D> for KdeDecorationState
where
    D: Dispatch<OrgKdeKwinServerDecoration, WlSurface> + KdeDecorationHandler + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        kde_decoration: &OrgKdeKwinServerDecoration,
        request: Request,
        surface: &WlSurface,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        let logger = &state.kde_decoration_state().logger;
        slog::trace!(
            logger,
            "Decoration request for surface {:?}: {:?}",
            surface,
            request
        );

        match request {
            Request::RequestMode { mode } => state.request_mode(surface, kde_decoration, mode),
            Request::Release => state.release(surface),
            _ => unreachable!(),
        }
    }
}
