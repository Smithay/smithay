//! Utilities for handling shell surfaces with the `wlr_layer_shell` protocol
//!
//! This interface should be suitable for the implementation of many desktop shell components,
//! and a broad number of other applications that interact with the desktop.
//!
//! ### Initialization
//!
//! To initialize this handler, create the [`WlrLayerShellState`], store it inside your `State` and
//! implement the [`WlrLayerShellHandler`], as shown in this example:
//!
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use smithay::delegate_layer_shell;
//! use smithay::wayland::shell::wlr_layer::{WlrLayerShellState, WlrLayerShellHandler, LayerSurface, Layer};
//! use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
//!
//! # struct State { layer_shell_state: WlrLayerShellState }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! let layer_shell_state = WlrLayerShellState::new::<State, _>(
//!     &display.handle(),
//!     None  // put a logger if you want
//! );
//!
//! // - Put your layer_shell_state into your `State`.
//! // - And implement `LayerShellHandler
//! impl WlrLayerShellHandler for State {
//!     fn shell_state(&mut self) -> &mut WlrLayerShellState {
//!         &mut self.layer_shell_state
//!     }
//!
//!     fn new_layer_surface(
//!         &mut self,
//!         surface: LayerSurface,
//!         output: Option<WlOutput>,
//!         layer: Layer,
//!         namespace: String,
//!     ) {
//!         # let _ = (surface, output, layer, namespace);
//!         // your implementation
//!     }
//! }
//! // let smithay implement wayland_server::DelegateDispatch
//! delegate_layer_shell!(State);
//!
//! // You're now ready to go!
//! ```

use std::sync::{Arc, Mutex};

use wayland_protocols_wlr::layer_shell::v1::server::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1,
};
use wayland_server::{
    backend::GlobalId,
    protocol::{wl_output::WlOutput, wl_surface},
    DisplayHandle, GlobalDispatch, Resource,
};

use crate::{
    utils::{alive_tracker::IsAlive, Logical, Serial, Size, SERIAL_COUNTER},
    wayland::{
        compositor::{self, Cacheable},
        shell::xdg,
    },
};

mod handlers;
mod types;

pub use handlers::WlrLayerSurfaceUserData;
pub use types::{Anchor, ExclusiveZone, KeyboardInteractivity, Layer, Margins};

/// The role of a wlr_layer_shell_surface
pub const LAYER_SURFACE_ROLE: &str = "zwlr_layer_surface_v1";

/// Data associated with XDG popup surface  
///
/// ```no_run
/// use smithay::wayland::compositor;
/// use smithay::wayland::shell::wlr_layer::LayerSurfaceData;
///
/// # let wl_surface = todo!();
/// compositor::with_states(&wl_surface, |states| {
///     states.data_map.get::<LayerSurfaceData>();
/// });
/// ```
pub type LayerSurfaceData = Mutex<LayerSurfaceAttributes>;

/// Attributes for layer surface
#[derive(Debug)]
pub struct LayerSurfaceAttributes {
    surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    /// Defines if the surface has received at least one
    /// layer_surface.ack_configure from the client
    pub configured: bool,
    /// The serial of the last acked configure
    pub configure_serial: Option<Serial>,
    /// Holds the state if the surface has sent the initial
    /// configure event to the client. It is expected that
    /// during the first commit a initial
    /// configure event is sent to the client
    pub initial_configure_sent: bool,
    /// Holds the configures the server has sent out
    /// to the client waiting to be acknowledged by
    /// the client. All pending configures that are older
    /// than the acknowledged one will be discarded during
    /// processing layer_surface.ack_configure.
    pending_configures: Vec<LayerSurfaceConfigure>,
    /// Holds the pending state as set by the server.
    pub server_pending: Option<LayerSurfaceState>,
    /// Holds the last server_pending state that has been acknowledged
    /// by the client. This state should be cloned to the current
    /// during a commit.
    pub last_acked: Option<LayerSurfaceState>,
    /// Holds the current state of the layer after a successful
    /// commit.
    pub current: LayerSurfaceState,
}

impl LayerSurfaceAttributes {
    fn new(surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1) -> Self {
        Self {
            surface,
            configured: false,
            configure_serial: None,
            initial_configure_sent: false,
            pending_configures: Vec::new(),
            server_pending: None,
            last_acked: None,
            current: Default::default(),
        }
    }

    fn ack_configure(&mut self, serial: Serial) -> Option<LayerSurfaceConfigure> {
        let configure = self
            .pending_configures
            .iter()
            .find(|configure| configure.serial == serial)
            .cloned()?;

        self.last_acked = Some(configure.state.clone());

        self.configured = true;
        self.configure_serial = Some(serial);
        self.pending_configures.retain(|c| c.serial > serial);
        Some(configure)
    }
}

/// State of a layer surface
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LayerSurfaceState {
    /// The suggested size of the surface
    pub size: Option<Size<i32, Logical>>,
}

/// Represents the client pending state
#[derive(Debug, Default, Clone, Copy)]
pub struct LayerSurfaceCachedState {
    /// The size requested by the client
    pub size: Size<i32, Logical>,
    /// Anchor bitflags, describing how the layers surface should be positioned and sized
    pub anchor: Anchor,
    /// Descripton of exclusive zone
    pub exclusive_zone: ExclusiveZone,
    /// Describes distance from the anchor point of the output
    pub margin: Margins,
    /// Describes how keyboard events are delivered to this surface
    pub keyboard_interactivity: KeyboardInteractivity,
    /// The layer that the surface is rendered on
    pub layer: Layer,
}

impl Cacheable for LayerSurfaceCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        *self
    }
    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

/// Shell global state
///
/// This state allows you to retrieve a list of surfaces
/// currently known to the shell global.
#[derive(Debug, Clone)]
pub struct WlrLayerShellState {
    known_layers: Arc<Mutex<Vec<LayerSurface>>>,
    shell_global: GlobalId,
    _log: slog::Logger,
}

impl WlrLayerShellState {
    /// Create a new `wlr_layer_shell` globals
    pub fn new<D, L>(display: &DisplayHandle, logger: L) -> WlrLayerShellState
    where
        L: Into<Option<::slog::Logger>>,
        D: GlobalDispatch<ZwlrLayerShellV1, ()>,
        D: 'static,
    {
        let log = crate::slog_or_fallback(logger);

        let shell_global = display.create_global::<D, ZwlrLayerShellV1, _>(4, ());

        WlrLayerShellState {
            known_layers: Default::default(),
            shell_global,
            _log: log.new(slog::o!("smithay_module" => "layer_shell_handler")),
        }
    }

    /// Get shell global id
    pub fn shell_global(&self) -> GlobalId {
        self.shell_global.clone()
    }

    /// Access all the shell surfaces known by this handler
    pub fn layer_surfaces(&self) -> impl DoubleEndedIterator<Item = LayerSurface> {
        self.known_layers.lock().unwrap().clone().into_iter()
    }
}

/// Handler for wlr layer shell
#[allow(unused_variables)]
pub trait WlrLayerShellHandler {
    /// [WlrLayerShellState] getter
    fn shell_state(&mut self) -> &mut WlrLayerShellState;

    /// A new layer surface was created
    ///
    /// You likely need to send a [`LayerSurfaceConfigure`] to the surface, to hint the
    /// client as to how its layer surface should be sized.
    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    );

    /// A new popup was assigned a layer surface as it's parent
    fn new_popup(&mut self, parent: LayerSurface, popup: xdg::PopupSurface) {}

    /// A surface has acknowledged a configure serial.
    fn ack_configure(&mut self, surface: wl_surface::WlSurface, configure: LayerSurfaceConfigure) {}

    /// A layer surface was destroyed.
    fn destroyed(&mut self) {}
}

/// A handle to a layer surface
#[derive(Debug, Clone)]
pub struct LayerSurface {
    wl_surface: wl_surface::WlSurface,
    shell_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
}

impl std::cmp::PartialEq for LayerSurface {
    fn eq(&self, other: &Self) -> bool {
        self.wl_surface == other.wl_surface
    }
}

impl LayerSurface {
    /// Checks if the surface is still alive
    pub fn alive(&self) -> bool {
        self.wl_surface.alive() && self.shell_surface.alive()
    }

    /// Gets the current pending state for a configure
    ///
    /// Returns `Some` if either no initial configure has been sent or
    /// the `server_pending` is `Some` and different from the last pending
    /// configure or `last_acked` if there is no pending
    ///
    /// Returns `None` if either no `server_pending` or the pending
    /// has already been sent to the client or the pending is equal
    /// to the `last_acked`
    fn get_pending_state(&self, attributes: &mut LayerSurfaceAttributes) -> Option<LayerSurfaceState> {
        if !attributes.initial_configure_sent {
            return Some(attributes.server_pending.take().unwrap_or_default());
        }

        let server_pending = match attributes.server_pending.take() {
            Some(state) => state,
            None => {
                return None;
            }
        };

        let last_state = attributes
            .pending_configures
            .last()
            .map(|c| &c.state)
            .or(attributes.last_acked.as_ref());

        if let Some(state) = last_state {
            if state == &server_pending {
                return None;
            }
        }

        Some(server_pending)
    }

    /// Send a configure event to this layer surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    ///
    /// You can manipulate the state that will be sent to the client with the [`with_pending_state`](#method.with_pending_state)
    /// method.
    pub fn send_configure(&self) {
        let configure = compositor::with_states(&self.wl_surface, |states| {
            let mut attributes = states
                .data_map
                .get::<Mutex<LayerSurfaceAttributes>>()
                .unwrap()
                .lock()
                .unwrap();
            if let Some(pending) = self.get_pending_state(&mut *attributes) {
                let configure = LayerSurfaceConfigure {
                    serial: SERIAL_COUNTER.next_serial(),
                    state: pending,
                };

                attributes.pending_configures.push(configure.clone());
                attributes.initial_configure_sent = true;

                Some(configure)
            } else {
                None
            }
        });

        // send surface configure
        if let Some(configure) = configure {
            let (width, height) = configure.state.size.unwrap_or_default().into();
            let serial = configure.serial;
            self.shell_surface
                .configure(serial.into(), width as u32, height as u32);
        }
    }

    /// Make sure this surface was configured
    ///
    /// Returns `true` if it was, if not, returns `false` and raise
    /// a protocol error to the associated layer surface. Also returns `false`
    /// if the surface is already destroyed.
    pub fn ensure_configured(&self) -> bool {
        let configured = compositor::with_states(&self.wl_surface, |states| {
            states
                .data_map
                .get::<Mutex<LayerSurfaceAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .configured
        });
        if !configured {
            self.shell_surface.post_error(
                zwlr_layer_shell_v1::Error::AlreadyConstructed,
                "layer_surface has never been configured",
            );
        }
        configured
    }

    /// Send a "close" event to the client
    pub fn send_close(&self) {
        self.shell_surface.closed()
    }

    /// Access the underlying `wl_surface` of this layer surface
    ///
    /// Returns `None` if the layer surface actually no longer exists.
    pub fn wl_surface(&self) -> &wl_surface::WlSurface {
        &self.wl_surface
    }

    /// Allows the pending state of this layer to
    /// be manipulated.
    ///
    /// This should be used to inform the client about size and state changes,
    /// for example after a resize request from the client.
    ///
    /// The state will be sent to the client when calling [`send_configure`](#method.send_configure).
    pub fn with_pending_state<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut LayerSurfaceState) -> T,
    {
        compositor::with_states(&self.wl_surface, |states| {
            let mut attributes = states
                .data_map
                .get::<Mutex<LayerSurfaceAttributes>>()
                .unwrap()
                .lock()
                .unwrap();
            if attributes.server_pending.is_none() {
                attributes.server_pending = Some(attributes.current.clone());
            }

            let server_pending = attributes.server_pending.as_mut().unwrap();
            f(server_pending)
        })
    }

    /// Gets a copy of the current state of this layer
    ///
    /// Returns `None` if the underlying surface has been
    /// destroyed
    pub fn current_state(&self) -> LayerSurfaceState {
        compositor::with_states(&self.wl_surface, |states| {
            let attributes = states
                .data_map
                .get::<Mutex<LayerSurfaceAttributes>>()
                .unwrap()
                .lock()
                .unwrap();

            attributes.current.clone()
        })
    }
}

/// A configure message for layer surfaces
#[derive(Debug, Clone)]
pub struct LayerSurfaceConfigure {
    /// The state associated with this configure
    pub state: LayerSurfaceState,

    /// A serial number to track ACK from the client
    ///
    /// This should be an ever increasing number, as the ACK-ing
    /// from a client for a serial will validate all pending lower
    /// serials.
    pub serial: Serial,
}

/// Macro to delegate implementation of wlr layer shell to [`WlrLayerShellState`].
///
/// You must also implement [`WlrLayerShellHandler`] to use this.
#[macro_export]
macro_rules! delegate_layer_shell {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        type __ZwlrLayerShellV1 =
            $crate::reexports::wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_shell_v1::ZwlrLayerShellV1;
        type __ZwlrLayerShellSurfaceV1 =
            $crate::reexports::wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __ZwlrLayerShellV1: ()
        ] => $crate::wayland::shell::wlr_layer::WlrLayerShellState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __ZwlrLayerShellSurfaceV1: $crate::wayland::shell::wlr_layer::WlrLayerSurfaceUserData
        ] => $crate::wayland::shell::wlr_layer::WlrLayerShellState);

        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __ZwlrLayerShellV1: ()
        ] => $crate::wayland::shell::wlr_layer::WlrLayerShellState);
    };
}
