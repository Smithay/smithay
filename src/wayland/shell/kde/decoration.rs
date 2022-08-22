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
//!
//! # struct State { kde_decoration_state: KdeDecorationState };
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! // Create the new KdeDecorationState.
//! let state = KdeDecorationState::new::<State, _>(&display.handle(), None);
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

use slog::Logger;
use wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::{
    Mode, OrgKdeKwinServerDecoration,
};
use wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::{
    Mode as DefaultMode, OrgKdeKwinServerDecorationManager,
};
use wayland_server::backend::GlobalId;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Dispatch, DisplayHandle, GlobalDispatch, WEnum};

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
    fn release(&mut self, _surface: &WlSurface) {}
}

/// KDE server decoration state.
#[derive(Debug)]
pub struct KdeDecorationState {
    pub(crate) default_mode: DefaultMode,
    pub(crate) logger: Logger,

    kde_decoration_manager: GlobalId,
}

impl KdeDecorationState {
    /// Create a new KDE server decoration global.
    pub fn new<D, L>(display: &DisplayHandle, default_mode: DefaultMode, logger: L) -> Self
    where
        D: GlobalDispatch<OrgKdeKwinServerDecorationManager, ()>
            + Dispatch<OrgKdeKwinServerDecorationManager, ()>
            + Dispatch<OrgKdeKwinServerDecoration, WlSurface>
            + KdeDecorationHandler
            + 'static,
        L: Into<Option<Logger>>,
    {
        let kde_decoration_manager = display.create_global::<D, OrgKdeKwinServerDecorationManager, _>(1, ());
        let logger =
            crate::slog_or_fallback(logger).new(slog::o!("smithay_module" => "kde_decoration_handler"));

        Self {
            kde_decoration_manager,
            default_mode,
            logger,
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
            $crate::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::OrgKdeKwinServerDecorationManager: ()
        ] => $crate::wayland::shell::kde::decoration::KdeDecorationState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::OrgKdeKwinServerDecorationManager: ()
        ] => $crate::wayland::shell::kde::decoration::KdeDecorationState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::OrgKdeKwinServerDecoration: $crate::reexports::wayland_server::protocol::wl_surface::WlSurface
        ] => $crate::wayland::shell::kde::decoration::KdeDecorationState);
    };
}
