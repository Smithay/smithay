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
//! let layer_shell_state = WlrLayerShellState::new::<State>(
//!     &display.handle(),
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

use tracing::{trace, trace_span};
use wayland_protocols_wlr::layer_shell::v1::server::{
    zwlr_layer_shell_v1::ZwlrLayerShellV1, zwlr_layer_surface_v1,
};
use wayland_server::{
    backend::GlobalId,
    protocol::{wl_output::WlOutput, wl_surface},
    Client, DisplayHandle, GlobalDispatch, Resource as _,
};

use crate::{
    utils::{alive_tracker::IsAlive, Logical, Serial, Size, SERIAL_COUNTER},
    wayland::{
        compositor::{self, BufferAssignment, Cacheable, SurfaceAttributes},
        shell::xdg,
    },
};

mod handlers;
mod types;

pub use handlers::WlrLayerSurfaceUserData;
pub use types::{Anchor, ExclusiveZone, KeyboardInteractivity, Layer, Margins};

/// The role of a wlr_layer_shell_surface
pub const LAYER_SURFACE_ROLE: &str = "zwlr_layer_surface_v1";

/// Data associated with layer surface
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
    /// Holds the last configure that has been acknowledged by the client. This state should be
    /// cloned to the current during a commit. Note that this state can be newer than the last
    /// acked state at the time of the last commit.
    pub last_acked: Option<LayerSurfaceConfigure>,
}

impl LayerSurfaceAttributes {
    fn new(surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1) -> Self {
        Self {
            surface,
            initial_configure_sent: false,
            pending_configures: Vec::new(),
            server_pending: None,
            last_acked: None,
        }
    }

    fn ack_configure(&mut self, serial: Serial) -> Option<LayerSurfaceConfigure> {
        let configure = self
            .pending_configures
            .iter()
            .find(|configure| configure.serial == serial)
            .cloned()?;

        self.last_acked = Some(configure.clone());

        self.pending_configures.retain(|c| c.serial > serial);
        Some(configure)
    }

    fn reset(&mut self) {
        self.initial_configure_sent = false;
        self.pending_configures = Vec::new();
        self.server_pending = None;
        self.last_acked = None;
    }

    fn current_server_state(&self) -> LayerSurfaceState {
        self.pending_configures
            .last()
            .map(|c| &c.state)
            .or(self.last_acked.as_ref().map(|c| &c.state))
            .cloned()
            .unwrap_or_default()
    }

    fn has_pending_changes(&self) -> bool {
        self.server_pending
            .as_ref()
            .map(|s| *s != self.current_server_state())
            .unwrap_or(false)
    }
}

/// State of a layer surface
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LayerSurfaceState {
    /// The suggested size of the surface
    pub size: Option<Size<i32, Logical>>,
}

/// Represents the client pending state
#[derive(Debug, Default, Clone)]
pub struct LayerSurfaceCachedState {
    /// The size requested by the client
    pub size: Size<i32, Logical>,
    /// Anchor bitflags, describing how the layers surface should be positioned and sized
    pub anchor: Anchor,
    /// Descripton of exclusive zone
    pub exclusive_zone: ExclusiveZone,
    /// Edge for exclusive zone
    pub exclusive_edge: Option<Anchor>,
    /// Describes distance from the anchor point of the output
    pub margin: Margins,
    /// Describes how keyboard events are delivered to this surface
    pub keyboard_interactivity: KeyboardInteractivity,
    /// The layer that the surface is rendered on
    pub layer: Layer,
    /// Configure last acknowledged by the client at the time of the commit.
    ///
    /// Reset to `None` when the surface unmaps.
    pub last_acked: Option<LayerSurfaceConfigure>,
}

impl Cacheable for LayerSurfaceCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        self.clone()
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
}

/// Data associated with a layer shell global
#[allow(missing_debug_implementations)]
pub struct WlrLayerShellGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

impl WlrLayerShellState {
    /// Create a new `wlr_layer_shell` global
    pub fn new<D>(display: &DisplayHandle) -> WlrLayerShellState
    where
        D: GlobalDispatch<ZwlrLayerShellV1, WlrLayerShellGlobalData>,
        D: 'static,
    {
        Self::new_with_filter::<D, _>(display, |_| true)
    }

    /// Create a new `wlr_layer_shell` global with a client filter
    pub fn new_with_filter<D, F>(display: &DisplayHandle, filter: F) -> WlrLayerShellState
    where
        D: GlobalDispatch<ZwlrLayerShellV1, WlrLayerShellGlobalData>,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let shell_global = display.create_global::<D, ZwlrLayerShellV1, WlrLayerShellGlobalData>(
            5,
            WlrLayerShellGlobalData {
                filter: Box::new(filter),
            },
        );

        WlrLayerShellState {
            known_layers: Default::default(),
            shell_global,
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
    fn layer_destroyed(&mut self, surface: LayerSurface) {}
}

/// A handle to a layer surface
#[derive(Debug, Clone)]
pub struct LayerSurface {
    wl_surface: wl_surface::WlSurface,
    shell_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
}

impl std::cmp::PartialEq for LayerSurface {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.wl_surface == other.wl_surface
    }
}

impl LayerSurface {
    /// Checks if the surface is still alive
    #[inline]
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
            return Some(
                attributes
                    .server_pending
                    .take()
                    .unwrap_or_else(|| attributes.current_server_state().clone()),
            );
        }

        // Check if the state really changed, it is possible
        // that with_pending_state has been called without
        // modifying the state.
        if !attributes.has_pending_changes() {
            return None;
        }

        attributes.server_pending.take()
    }

    /// Send a pending configure event to this layer surface to suggest it a new configuration
    ///
    /// If changes have occurred a configure event will be send to the clients and the serial will be returned
    /// (for tracking the configure in [`WlrLayerShellHandler::ack_configure`] if desired).
    /// If no changes occurred no event will be send and `None` will be returned.
    ///
    /// See [`send_configure`](LayerSurface::send_configure) and [`has_pending_changes`](LayerSurface::has_pending_changes)
    /// for more information.
    pub fn send_pending_configure(&self) -> Option<Serial> {
        if self.has_pending_changes() {
            Some(self.send_configure())
        } else {
            None
        }
    }

    /// Send a configure event to this layer surface to suggest it a new configuration
    ///
    /// The serial of this configure will be tracked waiting for the client to ACK it.
    ///
    /// You can manipulate the state that will be sent to the client with the [`with_pending_state`](#method.with_pending_state)
    /// method.
    ///
    /// Note: This will always send a configure event, if you intend to only send a configure event on changes take a look at
    /// [`send_pending_configure`](LayerSurface::send_pending_configure)
    pub fn send_configure(&self) -> Serial {
        let configure = compositor::with_states(&self.wl_surface, |states| {
            let mut attributes = states
                .data_map
                .get::<Mutex<LayerSurfaceAttributes>>()
                .unwrap()
                .lock()
                .unwrap();

            let state = self
                .get_pending_state(&mut attributes)
                .unwrap_or_else(|| attributes.current_server_state().clone());

            let configure = LayerSurfaceConfigure {
                serial: SERIAL_COUNTER.next_serial(),
                state,
            };

            attributes.pending_configures.push(configure.clone());
            attributes.initial_configure_sent = true;

            configure
        });

        // send surface configure
        let (width, height) = configure.state.size.unwrap_or_default().into();
        let serial = configure.serial;
        self.shell_surface
            .configure(serial.into(), width as u32, height as u32);
        serial
    }

    /// Send a "close" event to the client
    pub fn send_close(&self) {
        self.shell_surface.closed()
    }

    /// Access the underlying `wl_surface` of this layer surface
    ///
    /// Returns `None` if the layer surface actually no longer exists.
    #[inline]
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
                attributes.server_pending = Some(attributes.current_server_state().clone());
            }

            let server_pending = attributes.server_pending.as_mut().unwrap();
            f(server_pending)
        })
    }

    /// Tests this [`LayerSurface`] for pending changes
    ///
    /// Returns `true` if [`with_pending_state`](LayerSurface::with_pending_state) was used to manipulate the state
    /// and resulted in a different state or if the initial configure is still pending.
    pub fn has_pending_changes(&self) -> bool {
        compositor::with_states(&self.wl_surface, |states| {
            let attributes = states
                .data_map
                .get::<Mutex<LayerSurfaceAttributes>>()
                .unwrap()
                .lock()
                .unwrap();

            !attributes.initial_configure_sent || attributes.has_pending_changes()
        })
    }

    /// Provides access to the current committed cached state.
    pub fn with_cached_state<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&LayerSurfaceCachedState) -> T,
    {
        compositor::with_states(&self.wl_surface, |states| {
            let mut guard = states.cached_state.get::<LayerSurfaceCachedState>();
            f(guard.current())
        })
    }

    /// Provides access to the current committed state.
    ///
    /// This is the state that the client last acked before making the current commit.
    pub fn with_committed_state<F, T>(&self, f: F) -> T
    where
        F: FnOnce(Option<&LayerSurfaceState>) -> T,
    {
        self.with_cached_state(move |state| f(state.last_acked.as_ref().map(|c| &c.state)))
    }

    /// Handles the role specific commit error checking
    ///
    /// This should be called when the underlying WlSurface
    /// handles a wl_surface.commit request.
    pub(crate) fn pre_commit_hook<D: 'static>(
        _state: &mut D,
        _dh: &DisplayHandle,
        surface: &wl_surface::WlSurface,
    ) {
        let _span = trace_span!("layer-surface pre-commit", surface = %surface.id()).entered();

        compositor::with_states(surface, |states| {
            let mut role = states.data_map.get::<LayerSurfaceData>().unwrap().lock().unwrap();

            let mut guard_layer = states.cached_state.get::<LayerSurfaceCachedState>();
            let pending = guard_layer.pending();

            if pending.size.w == 0 && !pending.anchor.anchored_horizontally() {
                role.surface.post_error(
                    zwlr_layer_surface_v1::Error::InvalidSize,
                    "width 0 requested without setting left and right anchors",
                );
                return;
            }

            if pending.size.h == 0 && !pending.anchor.anchored_vertically() {
                role.surface.post_error(
                    zwlr_layer_surface_v1::Error::InvalidSize,
                    "height 0 requested without setting top and bottom anchors",
                );
                return;
            }

            if let Some(edge) = pending.exclusive_edge {
                if !pending.anchor.contains(edge) {
                    role.surface.post_error(
                        zwlr_layer_surface_v1::Error::InvalidExclusiveEdge,
                        "exclusive edge is not an anchor",
                    );
                    return;
                }
            }

            // The presence of last_acked always follows the buffer assignment because the
            // surface is not allowed to attach a buffer without acking the initial configure.
            let had_buffer_before = pending.last_acked.is_some();

            let mut guard_surface = states.cached_state.get::<SurfaceAttributes>();
            let has_buffer = match &guard_surface.pending().buffer {
                Some(BufferAssignment::NewBuffer(_)) => true,
                Some(BufferAssignment::Removed) => false,
                None => had_buffer_before,
            };
            // Need to check had_buffer_before in case the client attaches a null buffer for the
            // initial commit---we don't want to consider that as "got unmapped" and reset role.
            // Reproducer: waybar.
            let got_unmapped = had_buffer_before && !has_buffer;

            if has_buffer {
                let Some(last_acked) = role.last_acked.clone() else {
                    role.surface.post_error(
                        zwlr_layer_surface_v1::Error::InvalidSurfaceState,
                        "must ack the initial configure before attaching buffer",
                    );
                    return;
                };

                // The surface remains, or became mapped, track the last acked state.
                pending.last_acked = Some(last_acked);
            } else {
                // The surface remains, or became, unmapped, meaning that it's in the initial
                // configure stage.
                pending.last_acked = None;
            }

            if got_unmapped {
                trace!(
                    "got unmapped; resetting role and cached state; have {} pending configures",
                    role.pending_configures.len()
                );

                // All attributes are discarded when an xdg_surface is unmapped. Though, we keep
                // the list of pending configures because there's no way for a surface to tell
                // an in-flight configure apart from our next initial configure after unmapping.
                let pending_configures = std::mem::take(&mut role.pending_configures);
                role.reset();
                role.pending_configures = pending_configures;
                *guard_layer.pending() = Default::default();
            }
        });
    }

    /// Access the underlying `zwlr_layer_surface_v1` of this layer surface
    ///
    pub fn shell_surface(&self) -> &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1 {
        &self.shell_surface
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
        const _: () = {
            use $crate::{
                reexports::{
                    wayland_protocols_wlr::layer_shell::v1::server::{
                        zwlr_layer_shell_v1::ZwlrLayerShellV1, zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
                    },
                    wayland_server::{delegate_dispatch, delegate_global_dispatch},
                },
                wayland::shell::wlr_layer::{
                    WlrLayerShellGlobalData, WlrLayerShellState, WlrLayerSurfaceUserData,
                },
            };

            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ZwlrLayerShellV1: ()] => WlrLayerShellState
            );

            delegate_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ZwlrLayerSurfaceV1: WlrLayerSurfaceUserData] => WlrLayerShellState
            );

            delegate_global_dispatch!(
                $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?
                $ty: [ZwlrLayerShellV1: WlrLayerShellGlobalData] => WlrLayerShellState
            );
        };
    };
}
