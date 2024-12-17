//! Implementation of wp_tearing_control protocol
//!
//! ### Example
//!
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle};
//! use smithay::{
//!     delegate_tearing_control, delegate_compositor,
//!     wayland::compositor::{self, CompositorState, CompositorClientState, CompositorHandler},
//!     wayland::tearing_control::{TearingControlSurfaceCachedState, TearingControlState},
//! };
//!
//! pub struct State {
//!     compositor_state: CompositorState,
//! };
//! struct ClientState { compositor_state: CompositorClientState }
//! impl wayland_server::backend::ClientData for ClientState {}
//!
//! delegate_tearing_control!(State);
//! delegate_compositor!(State);
//!
//! impl CompositorHandler for State {
//!    fn compositor_state(&mut self) -> &mut CompositorState {
//!        &mut self.compositor_state
//!    }
//!
//!    fn client_compositor_state<'a>(&self, client: &'a wayland_server::Client) -> &'a CompositorClientState {
//!        &client.get_data::<ClientState>().unwrap().compositor_state    
//!    }
//!
//!    fn commit(&mut self, surface: &WlSurface) {
//!        compositor::with_states(&surface, |states| {
//!            let current = states.cached_state.current::<TearingControlSurfaceCachedState>();
//!            dbg!(current.presentation_hint());
//!        });
//!    }
//! }
//!
//! let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! let compositor_state = CompositorState::new::<State>(&display.handle());
//! TearingControlState::new::<State>(&display.handle());
//!
//! let state = State {
//!     compositor_state,
//! };
//! ```

use std::sync::{
    atomic::{self, AtomicBool},
    Mutex,
};

use wayland_protocols::wp::tearing_control::v1::server::{
    wp_tearing_control_manager_v1::WpTearingControlManagerV1,
    wp_tearing_control_v1::{self, WpTearingControlV1},
};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Dispatch, DisplayHandle, GlobalDispatch,
};

use super::compositor::Cacheable;

mod dispatch;

/// Data associated with WlSurface
/// Represents the client pending state
///
/// ```no_run
/// use smithay::wayland::compositor;
/// use smithay::wayland::tearing_control::TearingControlSurfaceCachedState;
///
/// # let wl_surface = todo!();
/// compositor::with_states(&wl_surface, |states| {
///     let current = states.cached_state.current::<TearingControlSurfaceCachedState>();
///     dbg!(current.presentation_hint());
/// });
/// ```
#[derive(Debug, Clone, Copy)]
pub struct TearingControlSurfaceCachedState {
    presentation_hint: wp_tearing_control_v1::PresentationHint,
}

impl TearingControlSurfaceCachedState {
    /// Provides information for if submitted frames from the client may be presented with tearing.
    pub fn presentation_hint(&self) -> &wp_tearing_control_v1::PresentationHint {
        &self.presentation_hint
    }
}

impl Default for TearingControlSurfaceCachedState {
    fn default() -> Self {
        Self {
            presentation_hint: wp_tearing_control_v1::PresentationHint::Vsync,
        }
    }
}

impl Cacheable for TearingControlSurfaceCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        *self
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

#[derive(Debug)]
struct TearingControlSurfaceData {
    is_resource_attached: AtomicBool,
}

impl TearingControlSurfaceData {
    fn new() -> Self {
        Self {
            is_resource_attached: AtomicBool::new(false),
        }
    }

    fn set_is_resource_attached(&self, is_attached: bool) {
        self.is_resource_attached
            .store(is_attached, atomic::Ordering::Release)
    }

    fn is_resource_attached(&self) -> bool {
        self.is_resource_attached.load(atomic::Ordering::Acquire)
    }
}

/// User data of [WpTearingControlV1] object
#[derive(Debug)]
pub struct TearingControlUserData(Mutex<WlSurface>);

impl TearingControlUserData {
    fn new(surface: WlSurface) -> Self {
        Self(Mutex::new(surface))
    }

    fn wl_surface(&self) -> WlSurface {
        self.0.lock().unwrap().clone()
    }
}

/// Delegate type for [WpTearingControlManagerV1] global.
#[derive(Debug)]
pub struct TearingControlState {
    global: GlobalId,
}

impl TearingControlState {
    /// Regiseter new [WpTearingControlManagerV1] global
    pub fn new<D>(display: &DisplayHandle) -> TearingControlState
    where
        D: GlobalDispatch<WpTearingControlManagerV1, ()>
            + Dispatch<WpTearingControlManagerV1, ()>
            + Dispatch<WpTearingControlV1, TearingControlUserData>
            + 'static,
    {
        let global = display.create_global::<D, WpTearingControlManagerV1, _>(1, ());

        TearingControlState { global }
    }

    /// Returns the [WpTearingControlManagerV1] global id
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// Macro to delegate implementation of the wp tearing control protocol
#[macro_export]
macro_rules! delegate_tearing_control {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        type __WpTearingControlManagerV1 =
            $crate::reexports::wayland_protocols::wp::tearing_control::v1::server::wp_tearing_control_manager_v1::WpTearingControlManagerV1;
        type __WpTearingControlV1 =
            $crate::reexports::wayland_protocols::wp::tearing_control::v1::server::wp_tearing_control_v1::WpTearingControlV1;

        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpTearingControlManagerV1: ()
            ] => $crate::wayland::tearing_control::TearingControlState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpTearingControlManagerV1: ()
            ] => $crate::wayland::tearing_control::TearingControlState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpTearingControlV1: $crate::wayland::tearing_control::TearingControlUserData
            ] => $crate::wayland::tearing_control::TearingControlState
        );
    };
}
