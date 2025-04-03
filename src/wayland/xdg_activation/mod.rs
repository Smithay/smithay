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
//!         }
//!     }
//! }
//!
//! // Delegate xdg activation handling for State to XdgActivationState.
//! delegate_xdg_activation!(State);
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//! let state = State {
//!     activation_state: XdgActivationState::new::<State>(&display_handle),
//! };
//!
//! // Rest of the compositor goes here...
//! ```

use std::{
    collections::HashMap,
    ops,
    sync::{atomic::AtomicBool, Arc, Mutex},
    time::Instant,
};

use wayland_protocols::xdg::activation::v1::server::xdg_activation_v1;
use wayland_server::{
    backend::{ClientId, GlobalId},
    protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
    Dispatch, DisplayHandle, GlobalDispatch,
};

use rand::distr::{Alphanumeric, SampleString};

use crate::utils::{user_data::UserDataMap, Serial};

mod dispatch;

/// Contains the unique string token of activation request
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct XdgActivationToken(String);

impl XdgActivationToken {
    fn new() -> Self {
        Self(Alphanumeric.sample_string(&mut rand::rng(), 32))
    }

    /// Extracts a string slice containing the entire token.
    #[inline]
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
    #[inline]
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<XdgActivationToken> for String {
    #[inline]
    fn from(s: XdgActivationToken) -> Self {
        s.0
    }
}

/// Activation data asosiated with the [`XdgActivationToken`]

#[derive(Debug, Clone)]
pub struct XdgActivationTokenData {
    /// Client that requested the token
    pub client_id: Option<ClientId>,
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
    /// You can use this to ignore tokens based on time.
    /// For example you could ignore all tokens older than 5s.
    pub timestamp: Instant,
    /// Additional user data attached
    pub user_data: Arc<UserDataMap>,
}

impl XdgActivationTokenData {
    fn new(
        client_id: Option<ClientId>,
        serial: Option<(Serial, WlSeat)>,
        app_id: Option<String>,
        surface: Option<WlSurface>,
    ) -> (XdgActivationToken, XdgActivationTokenData) {
        (
            XdgActivationToken::new(),
            XdgActivationTokenData {
                client_id,
                serial,
                app_id,
                surface,
                timestamp: Instant::now(),
                user_data: Arc::new(UserDataMap::new()),
            },
        )
    }
}

impl Default for XdgActivationTokenData {
    fn default() -> Self {
        Self {
            client_id: None,
            serial: None,
            app_id: None,
            surface: None,
            timestamp: Instant::now(),
            user_data: Arc::new(UserDataMap::new()),
        }
    }
}

/// Tracks the list of pending and current activation requests
#[derive(Debug)]
pub struct XdgActivationState {
    global: GlobalId,
    known_tokens: HashMap<XdgActivationToken, XdgActivationTokenData>,
}

impl XdgActivationState {
    /// Creates a new xdg activation global.
    ///
    /// In order to use this abstraction, your `D` type needs to implement [`XdgActivationHandler`].
    pub fn new<D>(display: &DisplayHandle) -> XdgActivationState
    where
        D: GlobalDispatch<xdg_activation_v1::XdgActivationV1, ()>
            + Dispatch<xdg_activation_v1::XdgActivationV1, ()>
            + XdgActivationHandler
            + 'static,
    {
        let global = display.create_global::<D, xdg_activation_v1::XdgActivationV1, _>(1, ());

        XdgActivationState {
            global,
            known_tokens: HashMap::new(),
        }
    }

    /// Create a token without any client association, e.g. for spawning processes from the compositor.
    ///
    /// This will not invoke [`XdgActivationHandler::token_created`] like client-created tokens,
    /// instead use the return arguments to handle any initialization of the data you might need and to copy the token.
    pub fn create_external_token(
        &mut self,
        data: impl Into<Option<XdgActivationTokenData>>,
    ) -> (&XdgActivationToken, &XdgActivationTokenData) {
        let token = XdgActivationToken::new();
        let data = data.into().unwrap_or_default();
        self.known_tokens.insert(token.clone(), data);
        self.known_tokens.get_key_value(&token).unwrap()
    }

    /// Iterate over all known tokens and their associated data
    pub fn tokens(&self) -> impl Iterator<Item = (&XdgActivationToken, &XdgActivationTokenData)> {
        self.known_tokens.iter()
    }

    /// Access the data of a known token
    pub fn data_for_token(&self, token: &XdgActivationToken) -> Option<&XdgActivationTokenData> {
        self.known_tokens.get(token)
    }

    /// Retain pending tokens
    ///
    /// You may want to remove super old tokens
    /// that were never turned into activation request for some reason
    pub fn retain_tokens<F>(&mut self, mut f: F)
    where
        F: FnMut(&XdgActivationToken, &XdgActivationTokenData) -> bool,
    {
        self.known_tokens.retain(|k, v| f(k, v))
    }

    /// Removes an activation token from the internal storage.
    ///
    /// Returns `true` if the token was found and subsequently removed.
    pub fn remove_token(&mut self, token: &XdgActivationToken) -> bool {
        self.known_tokens.remove(token).is_some()
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

    /// A client has created a new token.
    ///
    /// If the token isn't considered valid, it can be immediately untracked by returning `false`.
    /// The default implementation considers every token valid and will always return `true`.
    ///
    /// This method may also be used to attach user_data to the token.
    fn token_created(&mut self, token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        let _ = (token, data);
        true
    }

    /// A client has requested surface activation.
    ///
    /// The compositor may know which client requested this by checking the token data and may decide whether
    /// or not to follow through with the activation if it's considered unwanted.
    ///
    /// The token remains in the pool and might be used to issue other requests
    /// until the compositor decides to remove it using [`XdgActivationState::remove_token`] or [`XdgActivationState::retain_tokens`].
    fn request_activation(
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
