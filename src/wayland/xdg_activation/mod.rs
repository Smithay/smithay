//! Utilities for handling activation requests with the `xdg_activation` protocol
//!
//! ### Example
//!
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle};
//! use smithay::{
//!     delegate_xdg_activation,
//!     wayland::xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData}
//! };
//!
//! pub struct State {
//!     activation_state: XdgActivationState,
//! }
//!
//! impl XdgActivationHandler for State {
//!     fn activation_state(&mut self) -> &mut XdgActivationState {
//!         &mut self.activation_state
//!     }
//!
//!     fn request_activation(
//!         &mut self,
//!         token: XdgActivationToken,
//!         token_data: XdgActivationTokenData,
//!         surface: WlSurface
//!     ) {
//!         if token_data.timestamp.elapsed().as_secs() < 10 {
//!             // Request surface activation
//!         } else{
//!             // Discard the request
//!             self.activation_state.remove_request(&token);
//!         }
//!     }
//!
//!     fn destroy_activation(
//!         &mut self,
//!         token: XdgActivationToken,
//!         token_data: XdgActivationTokenData,
//!         surface: WlSurface
//!     ) {
//!         // The request is cancelled
//!     }
//! }
//!
//! // Delegate xdg activation handling for State to XdgActivationState.
//! delegate_xdg_activation!(State);
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//! let state = State {
//!     activation_state: XdgActivationState::new::<State, _>(&display_handle, None),
//! };
//!
//! // Rest of the compositor goes here...
//! ```

use std::{
    collections::HashMap,
    ops,
    sync::{atomic::AtomicBool, Mutex},
    time::Instant,
};

use wayland_protocols::xdg::activation::v1::server::xdg_activation_v1;
use wayland_server::{
    backend::GlobalId,
    protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
    Dispatch, DisplayHandle, GlobalDispatch,
};

use rand::distributions::{Alphanumeric, DistString};

use crate::wayland::Serial;

mod dispatch;

/// Contains the unique string token of activation request
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct XdgActivationToken(String);

impl XdgActivationToken {
    fn new() -> Self {
        Self(Alphanumeric.sample_string(&mut rand::thread_rng(), 32))
    }

    /// Extracts a string slice containing the entire token.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl ops::Deref for XdgActivationToken {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        &self.0
    }
}

impl From<String> for XdgActivationToken {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<XdgActivationToken> for String {
    fn from(s: XdgActivationToken) -> Self {
        s.0
    }
}

/// Activation data asosiated with the [`XdgActivationToken`]

#[derive(Debug, Clone)]
pub struct XdgActivationTokenData {
    /// Provides information about the seat and serial event that requested the token.
    ///
    /// The serial can come from an input or focus event.
    /// For instance, if a click triggers the launch of a third-party client,
    /// this field should contain serial and seat from the wl_pointer.button event.
    ///
    /// Some compositors might refuse to activate toplevels
    /// when the token doesn't have a valid and recent enough event serial.
    pub serial: Option<(Serial, WlSeat)>,
    /// The requesting client can specify an app_id to associate the token being created with it.
    pub app_id: Option<String>,
    /// The surface requesting the activation.
    ///
    /// Note, this is different from the surface that will be activated.
    pub surface: Option<WlSurface>,
    /// Timestamp of the token
    ///
    /// You can use this do ignore tokens based on time.
    /// For example you coould ignore all tokens older that 5s.
    pub timestamp: Instant,
}

impl XdgActivationTokenData {
    fn new(
        serial: Option<(Serial, WlSeat)>,
        app_id: Option<String>,
        surface: Option<WlSurface>,
    ) -> (XdgActivationToken, XdgActivationTokenData) {
        (
            XdgActivationToken::new(),
            XdgActivationTokenData {
                serial,
                app_id,
                surface,
                timestamp: Instant::now(),
            },
        )
    }
}

/// Tracks the list of pending and current activation requests
#[derive(Debug)]
pub struct XdgActivationState {
    _logger: ::slog::Logger,
    global: GlobalId,
    pending_tokens: HashMap<XdgActivationToken, XdgActivationTokenData>,
    activation_requests: HashMap<XdgActivationToken, (XdgActivationTokenData, WlSurface)>,
}

impl XdgActivationState {
    /// Creates a new xdg activation global.
    ///
    /// In order to use this abstraction, your `D` type needs to implement [`XdgActivationHandler`].
    pub fn new<D, L>(display: &DisplayHandle, logger: L) -> XdgActivationState
    where
        D: GlobalDispatch<xdg_activation_v1::XdgActivationV1, ()>
            + Dispatch<xdg_activation_v1::XdgActivationV1, ()>
            + XdgActivationHandler
            + 'static,
        L: Into<Option<::slog::Logger>>,
    {
        let logger = crate::slog_or_fallback(logger);
        let global = display.create_global::<D, xdg_activation_v1::XdgActivationV1, _>(1, ());

        XdgActivationState {
            _logger: logger.new(slog::o!("smithay_module" => "xdg_activation_handler")),
            global,
            pending_tokens: HashMap::new(),
            activation_requests: HashMap::new(),
        }
    }

    /// Get current activation requests
    ///
    /// HashMap contains token data and target surface.
    pub fn requests(&self) -> &HashMap<XdgActivationToken, (XdgActivationTokenData, WlSurface)> {
        &self.activation_requests
    }

    /// Remove and return the activation request
    ///
    /// If you consider a request to be unwanted you can use this method to
    /// discard it and don't track it any futher.
    pub fn remove_request(
        &mut self,
        token: &XdgActivationToken,
    ) -> Option<(XdgActivationTokenData, WlSurface)> {
        self.activation_requests.remove(token)
    }

    /// Retain activation requests
    pub fn retain_requests<F>(&mut self, mut f: F)
    where
        F: FnMut(&XdgActivationToken, &(XdgActivationTokenData, WlSurface)) -> bool,
    {
        self.activation_requests.retain(|k, v| f(k, v))
    }

    /// Retain pending tokens
    ///
    /// You may want to remove super old tokens
    /// that were never turned into activation request for some reason
    pub fn retain_pending_tokens<F>(&mut self, mut f: F)
    where
        F: FnMut(&XdgActivationToken, &XdgActivationTokenData) -> bool,
    {
        self.pending_tokens.retain(|k, v| f(k, v))
    }

    /// Returns the xdg activation global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// A trait implemented to be notified of activation requests using the xdg activation protocol.
pub trait XdgActivationHandler {
    /// Returns the activation state.
    fn activation_state(&mut self) -> &mut XdgActivationState;

    /// A client has requested surface activation.
    ///
    /// The compositor may know which client requested this by checking the token data and may decide whether
    /// or not to follow through with the activation if it's considered unwanted.
    ///
    /// If a request is unwanted, you can discard the request using [`XdgActivationState::remove_request`] to
    /// ignore any future requests.
    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    );

    /// The activation token was destroyed.
    ///
    /// The compositor may cancel any activation requests coming from the token.
    ///
    /// For example if your compositor blinks or highlights a window when it requests activation then the
    /// animation should stop when this function is called.
    fn destroy_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    );
}

/// Data assoicated with an activation token protocol object.
#[derive(Debug)]
pub struct ActivationTokenData {
    constructed: AtomicBool,
    build: Mutex<TokenBuilder>,
    token: Mutex<Option<XdgActivationToken>>,
}

/// Macro to delegate implementation of the xdg activation to [`XdgActivationState`].
///
/// You must also implement [`XdgActivationHandler`] to use this.
#[macro_export]
macro_rules! delegate_xdg_activation {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        type __XdgActivationV1 =
            $crate::reexports::wayland_protocols::xdg::activation::v1::server::xdg_activation_v1::XdgActivationV1;
        type __XdgActivationTokenV1 =
            $crate::reexports::wayland_protocols::xdg::activation::v1::server::xdg_activation_token_v1::XdgActivationTokenV1;

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __XdgActivationV1: ()
        ] => $crate::wayland::xdg_activation::XdgActivationState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __XdgActivationTokenV1: $crate::wayland::xdg_activation::ActivationTokenData
        ] => $crate::wayland::xdg_activation::XdgActivationState);

        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __XdgActivationV1: ()
            ] => $crate::wayland::xdg_activation::XdgActivationState
        );
    };
}

#[derive(Debug)]
struct TokenBuilder {
    serial: Option<(Serial, WlSeat)>,
    app_id: Option<String>,
    surface: Option<WlSurface>,
}
