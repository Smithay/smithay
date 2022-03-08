//! XDG Window decoration manager
//!
//! This interface allows a compositor to announce support for server-side decorations.
//!
//! A client can use this protocol to request being decorated by a supporting compositor.
//!
//!
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use smithay::wayland::shell::xdg::decoration::{init_xdg_decoration_manager, XdgDecorationRequest};
//! use smithay::reexports::wayland_protocols::unstable::xdg_decoration::v1::server::zxdg_toplevel_decoration_v1::Mode;
//!
//! # let mut display = wayland_server::Display::new();
//!
//! init_xdg_decoration_manager(
//!     &mut display,
//!     |req, _ddata| match req {
//!         XdgDecorationRequest::NewToplevelDecoration { toplevel } => {
//!             let res = toplevel.with_pending_state(|state| {
//!                   // Advertise server side decoration
//!                 state.decoration_mode = Some(Mode::ServerSide);
//!             });
//!
//!             if res.is_ok() {
//!                 toplevel.send_configure();
//!             }
//!         }
//!         XdgDecorationRequest::SetMode { .. } => {}
//!         XdgDecorationRequest::UnsetMode { .. } => {}
//!     },
//!     None,
//! );
//!

// TODO: Describe how to change decoration mode.

use wayland_protocols::unstable::xdg_decoration::v1::server::{
    zxdg_decoration_manager_v1,
    zxdg_toplevel_decoration_v1::{self, Mode},
};
use wayland_server::{
    backend::GlobalId, Client, DataInit, DelegateDispatch, DelegateDispatchBase, DelegateGlobalDispatch,
    DelegateGlobalDispatchBase, Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

use super::{ToplevelSurface, XdgShellHandler};
use crate::wayland::shell::xdg::XdgShellSurfaceUserData;

/// Delegate type for handling xdg decoration events.
#[derive(Debug)]
pub struct XdgDecorationManager {
    _logger: ::slog::Logger,
}

impl XdgDecorationManager {
    /// Creates a new delegate type for handling xdg decoration events.
    ///
    /// A global id is also returned to allow destroying the global in the future.
    pub fn new<D, L>(display: &mut Display<D>, logger: L) -> (XdgDecorationManager, GlobalId)
    where
        D: GlobalDispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, GlobalData = ()>
            + Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, UserData = ()>
            + 'static,
        L: Into<Option<::slog::Logger>>,
    {
        let _logger = crate::slog_or_fallback(logger);
        let global = display.create_global::<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>(1, ());

        (XdgDecorationManager { _logger }, global)
    }
}

/// Handler trait for xdg decoration events.
pub trait XdgDecorationHandler {
    /// Notification the client supports server side decoration on the toplevel.
    fn new_decoration(&mut self, dh: &mut DisplayHandle<'_>, toplevel: ToplevelSurface);

    /// Notification the client prefers the provided decoration decoration mode on the toplevel.
    fn request_mode(&mut self, dh: &mut DisplayHandle<'_>, toplevel: ToplevelSurface, mode: Mode);

    /// Notification the client does not prefer a particular decoration mode on the toplevel.
    fn unset_mode(&mut self, dh: &mut DisplayHandle<'_>, toplevel: ToplevelSurface);
}

/// Macro to delegate implementation of the xdg decoration to [`XdgDecorationManager`].
///
/// You must also implement [`XdgDecorationHandler`] to use this.
#[macro_export]
macro_rules! delegate_xdg_decoration {
    ($ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($ty: [
            wayland_protocols::unstable::xdg_decoration::v1::server::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1
        ] => $crate::wayland::shell::xdg::decoration::XdgDecorationManager);

        $crate::reexports::wayland_server::delegate_dispatch!($ty: [
            wayland_protocols::unstable::xdg_decoration::v1::server::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
            wayland_protocols::unstable::xdg_decoration::v1::server::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1
        ] => $crate::wayland::shell::xdg::decoration::XdgDecorationManager);
    };
}

pub(super) fn send_decoration_configure(
    dh: &mut DisplayHandle<'_>,
    id: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    mode: Mode,
) {
    id.configure(dh, mode)
}

impl DelegateGlobalDispatchBase<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>
    for XdgDecorationManager
{
    type GlobalData = ();
}

impl<D> DelegateGlobalDispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, D>
    for XdgDecorationManager
where
    D: GlobalDispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, GlobalData = Self::GlobalData>
        + Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, UserData = ()>
        + Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, UserData = ToplevelSurface>
        + XdgShellHandler
        + XdgDecorationHandler
        + 'static,
{
    fn bind(
        _: &mut D,
        _: &mut DisplayHandle<'_>,
        _: &Client,
        resource: New<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
        _: &Self::GlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl DelegateDispatchBase<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1> for XdgDecorationManager {
    type UserData = ();
}

impl<D> DelegateDispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, D> for XdgDecorationManager
where
    D: Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, UserData = Self::UserData>
        + Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, UserData = ToplevelSurface>
        + XdgShellHandler
        + XdgDecorationHandler
        + 'static,
{
    fn request(
        state: &mut D,
        _: &Client,
        resource: &zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
        request: zxdg_decoration_manager_v1::Request,
        _: &Self::UserData,
        dh: &mut DisplayHandle<'_>,
        data_init: &mut DataInit<'_, D>,
    ) {
        use self::zxdg_decoration_manager_v1::Request;

        match request {
            Request::GetToplevelDecoration { id, toplevel } => {
                let data = toplevel.data::<XdgShellSurfaceUserData>().unwrap();

                let mut decoration_guard = data.decoration.lock().unwrap();

                if decoration_guard.is_some() {
                    resource.post_error(
                        dh,
                        zxdg_toplevel_decoration_v1::Error::AlreadyConstructed,
                        "toplevel decoration is already constructed",
                    );
                    return;
                }

                let toplevel = state.xdg_shell_state().get_toplevel(&toplevel).unwrap();
                let toplevel_decoration = data_init.init(id, toplevel.clone());

                *decoration_guard = Some(toplevel_decoration);
                drop(decoration_guard);

                state.new_decoration(dh, toplevel);
            }

            Request::Destroy => {}

            _ => unreachable!(),
        }
    }
}

// zxdg_toplevel_decoration_v1

impl DelegateDispatchBase<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1> for XdgDecorationManager {
    type UserData = ToplevelSurface;
}

impl<D> DelegateDispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, D> for XdgDecorationManager
where
    D: Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, UserData = Self::UserData>
        + XdgDecorationHandler,
{
    fn request(
        state: &mut D,
        _: &Client,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        request: zxdg_toplevel_decoration_v1::Request,
        data: &Self::UserData,
        dh: &mut DisplayHandle<'_>,
        _: &mut DataInit<'_, D>,
    ) {
        use self::zxdg_toplevel_decoration_v1::Request;

        match request {
            Request::SetMode { mode } => {
                if let WEnum::Value(mode) = mode {
                    state.request_mode(dh, data.clone(), mode);
                }
            }

            Request::UnsetMode => {
                state.unset_mode(dh, data.clone());
            }

            Request::Destroy => {
                if let Some(data) = data.xdg_toplevel().data::<XdgShellSurfaceUserData>() {
                    data.decoration.lock().unwrap().take();
                }
            }

            _ => unreachable!(),
        }
    }
}
