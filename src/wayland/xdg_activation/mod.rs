//! Utilities for handling activation requests with the `xdg_activation` protocol
//!
//!
//! ### Example
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use smithay::wayland::xdg_activation::{init_xdg_activation_global, XdgActivationEvent};
//!
//! # let mut display = wayland_server::Display::new();
//! let (state, _) = init_xdg_activation_global(
//!     &mut display,
//!     // your implementation
//!     |state, req, dispatch_data| {
//!         match req{
//!             XdgActivationEvent::RequestActivation { token, token_data, surface } => {
//!                 if token_data.timestamp.elapsed().as_secs() < 10 {
//!                     // Request surface activation
//!                 } else{
//!                     // Discard the request
//!                     state.lock().unwrap().remove_request(&token);
//!                 }
//!             },
//!             XdgActivationEvent::DestroyActivationRequest {..} => {
//!                 // The request is canceled
//!             },
//!         }
//!     },
//!     None  // put a logger if you want
//! );
//! ```

use std::{
    cell::RefCell,
    collections::HashMap,
    ops,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Instant,
};

use wayland_protocols::staging::xdg_activation::v1::server::xdg_activation_v1;
use wayland_server::{
    protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
    DispatchData, Display, Filter, Global, Main, UserDataMap,
};

use rand::distributions::{Alphanumeric, DistString};

use crate::wayland::Serial;

mod handlers;

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
    _log: ::slog::Logger,
    user_data: UserDataMap,

    pending_tokens: HashMap<XdgActivationToken, XdgActivationTokenData>,

    activation_requests: HashMap<XdgActivationToken, (XdgActivationTokenData, WlSurface)>,
}

impl XdgActivationState {
    /// Get current activation requests
    ///
    /// HashMap contains token data and target surface
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

    /// Access the `UserDataMap` associated with this `XdgActivationState `
    pub fn user_data(&self) -> &UserDataMap {
        &self.user_data
    }
}

/// Creates new `xdg-activation` global.
pub fn init_xdg_activation_global<L, Impl>(
    display: &mut Display,
    implementation: Impl,
    logger: L,
) -> (
    Arc<Mutex<XdgActivationState>>,
    Global<xdg_activation_v1::XdgActivationV1>,
)
where
    L: Into<Option<::slog::Logger>>,
    Impl: FnMut(&Mutex<XdgActivationState>, XdgActivationEvent, DispatchData<'_>) + 'static,
{
    let log = crate::slog_or_fallback(logger);

    let implementation = Rc::new(RefCell::new(implementation));

    let activation_state = Arc::new(Mutex::new(XdgActivationState {
        _log: log.new(slog::o!("smithay_module" => "xdg_activation_handler")),
        user_data: UserDataMap::new(),
        pending_tokens: HashMap::new(),
        activation_requests: HashMap::new(),
    }));

    let state = activation_state.clone();
    let global = display.create_global(
        1,
        Filter::new(
            move |(global, _version): (Main<xdg_activation_v1::XdgActivationV1>, _), _, _| {
                handlers::implement_activation_global(global, state.clone(), implementation.clone());
            },
        ),
    );

    (activation_state, global)
}

/// Xdg activation related events
#[derive(Debug)]
pub enum XdgActivationEvent {
    /// Requests surface activation.
    ///
    /// The compositor may know who requested this by checking the token data
    /// and might decide not to follow through with the activation if it's considered unwanted.
    ///
    /// If you consider a request to be unwanted you can use [`XdgActivationState::remove_request`]
    /// to discard it and don't track it any futher.
    RequestActivation {
        /// Token of the request
        token: XdgActivationToken,
        /// Data asosiated with the token
        token_data: XdgActivationTokenData,
        /// Target surface
        surface: WlSurface,
    },
    /// The activation token just got destroyed
    ///
    /// In response to that activation request should be canceled.
    ///
    /// For example if your compostior blinks a window when it requests activation,
    /// after this request the animation should stop.
    DestroyActivationRequest {
        /// Token of the request that just died
        token: XdgActivationToken,
        /// Data asosiated with the token
        token_data: XdgActivationTokenData,
        /// Target surface
        surface: WlSurface,
    },
}
