//! KDE Window decoration manager
//!
//! This interface allows a compositor to announce support for KDE's legacy server-side decorations.
//!
//! A client can use this protocol to request being decorated by a supporting compositor.
//!
//! ```
//! extern crate wayland_server;
//! extern crate smithay;
//!
//! use smithay::delegate_kde_decoration;
//! use smithay::wayland::shell::kde::decoration::{KdeDecorationHandler, KdeDecorationState};
//! use wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::Mode;
//!
//! # struct State { kde_decoration_state: KdeDecorationState };
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! // Create the new KdeDecorationState.
//! let state = KdeDecorationState::new::<State>(&display.handle(), Mode::Server);
//!
//! // Insert KdeDecorationState into your compositor state.
//! // …
//!
//! // Implement KDE server decoration handlers.
//! impl KdeDecorationHandler for State {
//!     fn kde_decoration_state(&self) -> &KdeDecorationState {
//!         &self.kde_decoration_state
//!     }
//! }
//!
//! delegate_kde_decoration!(State);
//! ```

use wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::{
    Mode, OrgKdeKwinServerDecoration,
};
use wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::{
    Mode as DefaultMode, OrgKdeKwinServerDecorationManager,
};
use wayland_server::backend::GlobalId;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, Dispatch, DisplayHandle, GlobalDispatch, WEnum};

/// KDE server decoration handler.
pub trait KdeDecorationHandler {
    /// Return the KDE server decoration state.
    fn kde_decoration_state(&self) -> &KdeDecorationState;

    /// Handle new decoration object creation.
    ///
    /// Called whenever a new decoration object is created, usually this happens when a new window
    /// is opened.
    fn new_decoration(&mut self, _surface: &WlSurface, _decoration: &OrgKdeKwinServerDecoration) {}

    /// Handle surface decoration mode requests.
    ///
    /// Called when a surface requests a specific decoration mode or acknowledged the compositor's
    /// decoration request.
    ///
    /// **It is up to the compositor to prevent feedback loops**, a client is free to ignore modes
    /// suggested by [`OrgKdeKwinServerDecoration::mode`] and instead request their preferred mode
    /// instead.
    fn request_mode(
        &mut self,
        _surface: &WlSurface,
        decoration: &OrgKdeKwinServerDecoration,
        mode: WEnum<Mode>,
    ) {
        if let WEnum::Value(mode) = mode {
            decoration.mode(mode);
        }
    }

    /// Handle decoration object removal for a surface.
    fn release(&mut self, _decoration: &OrgKdeKwinServerDecoration, _surface: &WlSurface) {}
}

/// KDE server decoration state.
#[derive(Debug)]
pub struct KdeDecorationState {
    pub(crate) default_mode: DefaultMode,

    kde_decoration_manager: GlobalId,
}

/// Data associated with a KdeDecorationManager global.
#[allow(missing_debug_implementations)]
pub struct KdeDecorationManagerGlobalData {
    pub(crate) filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl KdeDecorationState {
    /// Create a new KDE server decoration global.
    pub fn new<D>(display: &DisplayHandle, default_mode: DefaultMode) -> Self
    where
        D: GlobalDispatch<OrgKdeKwinServerDecorationManager, KdeDecorationManagerGlobalData>
            + Dispatch<OrgKdeKwinServerDecorationManager, ()>
            + Dispatch<OrgKdeKwinServerDecoration, WlSurface>
            + KdeDecorationHandler
            + 'static,
    {
        Self::new_with_filter::<D, _>(display, default_mode, |_| true)
    }

    /// Create a new KDE server decoration global with a filter.
    ///
    /// Filters can be used to limit visibility of a global to certain clients.
    pub fn new_with_filter<D, F>(display: &DisplayHandle, default_mode: DefaultMode, filter: F) -> Self
    where
        D: GlobalDispatch<OrgKdeKwinServerDecorationManager, KdeDecorationManagerGlobalData>
            + Dispatch<OrgKdeKwinServerDecorationManager, ()>
            + Dispatch<OrgKdeKwinServerDecoration, WlSurface>
            + KdeDecorationHandler
            + 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let data = KdeDecorationManagerGlobalData {
            filter: Box::new(filter),
        };
        let kde_decoration_manager =
            display.create_global::<D, OrgKdeKwinServerDecorationManager, _>(1, data);

        Self {
            kde_decoration_manager,
            default_mode,
        }
    }

    /// Returns the id of the [`OrgKdeKwinServerDecorationManager`] global.
    pub fn global(&self) -> GlobalId {
        self.kde_decoration_manager.clone()
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_kde_decoration {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::OrgKdeKwinServerDecorationManager: $crate::wayland::shell::kde::decoration::KdeDecorationManagerGlobalData
        ] => $crate::wayland::shell::kde::decoration::KdeDecorationState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::OrgKdeKwinServerDecorationManager: ()
        ] => $crate::wayland::shell::kde::decoration::KdeDecorationState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::OrgKdeKwinServerDecoration: $crate::reexports::wayland_server::protocol::wl_surface::WlSurface
        ] => $crate::wayland::shell::kde::decoration::KdeDecorationState);
    };
}
