//! Implementation of `wp_alpha_modifier` protocol
//!
//! [`WaylandSurfaceRenderElement`][`crate::backend::renderer::element::surface::WaylandSurfaceRenderElement`]
//! takes the alpha multiplier into account automatically.
//!
//! ### Example
//!
//! ```no_run
//! # extern crate wayland_server;
//! #
//! use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle};
//! use smithay::{
//!     delegate_alpha_modifier, delegate_compositor,
//!     wayland::compositor::{self, CompositorState, CompositorClientState, CompositorHandler},
//!     wayland::alpha_modifier::{AlphaModifierSurfaceCachedState, AlphaModifierState},
//! };
//!
//! pub struct State {
//!     compositor_state: CompositorState,
//! };
//! struct ClientState { compositor_state: CompositorClientState }
//! impl wayland_server::backend::ClientData for ClientState {}
//!
//! delegate_alpha_modifier!(State);
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
//!            let current = states.cached_state.current::<AlphaModifierSurfaceCachedState>();
//!            dbg!(current.multiplier());
//!        });
//!    }
//! }
//!
//! let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! let compositor_state = CompositorState::new::<State>(&display.handle());
//! AlphaModifierState::new::<State>(&display.handle());
//!
//! let state = State {
//!     compositor_state,
//! };
//! ```

use std::sync::{
    atomic::{self, AtomicBool},
    Mutex,
};

use wayland_protocols::wp::alpha_modifier::v1::server::{
    wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1, wp_alpha_modifier_v1::WpAlphaModifierV1,
};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Dispatch, DisplayHandle, GlobalDispatch, Resource,
    Weak,
};

use super::compositor::Cacheable;

mod dispatch;

/// Data associated with WlSurface
/// Represents the client pending state
///
/// ```no_run
/// use smithay::wayland::compositor;
/// use smithay::wayland::alpha_modifier::AlphaModifierSurfaceCachedState;
///
/// # let wl_surface = todo!();
/// compositor::with_states(&wl_surface, |states| {
///     let current = states.cached_state.current::<AlphaModifierSurfaceCachedState>();
///     dbg!(current.multiplier());
/// });
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct AlphaModifierSurfaceCachedState {
    multiplier: Option<u32>,
}

impl AlphaModifierSurfaceCachedState {
    /// Alpha multiplier for the surface
    pub fn multiplier(&self) -> Option<u32> {
        self.multiplier
    }

    /// Alpha multiplier for the surface
    pub fn multiplier_f32(&self) -> Option<f32> {
        self.multiplier
            .map(|multiplier| multiplier as f32 / u32::MAX as f32)
    }

    /// Alpha multiplier for the surface
    pub fn multiplier_f64(&self) -> Option<f64> {
        self.multiplier
            .map(|multiplier| multiplier as f64 / u32::MAX as f64)
    }
}

impl Cacheable for AlphaModifierSurfaceCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        *self
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

#[derive(Debug)]
struct AlphaModifierSurfaceData {
    is_resource_attached: AtomicBool,
}

impl AlphaModifierSurfaceData {
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

/// User data of [WpAlphaModifierSurfaceV1] object
#[derive(Debug)]
pub struct AlphaModifierSurfaceUserData(Mutex<Weak<WlSurface>>);

impl AlphaModifierSurfaceUserData {
    fn new(surface: WlSurface) -> Self {
        Self(Mutex::new(surface.downgrade()))
    }

    fn wl_surface(&self) -> Option<WlSurface> {
        self.0.lock().unwrap().upgrade().ok()
    }
}

/// Delegate type for [WpAlphaModifierV1] global.
#[derive(Debug)]
pub struct AlphaModifierState {
    global: GlobalId,
}

impl AlphaModifierState {
    /// Regiseter new [WpAlphaModifierV1] global
    pub fn new<D>(display: &DisplayHandle) -> AlphaModifierState
    where
        D: GlobalDispatch<WpAlphaModifierV1, ()>
            + Dispatch<WpAlphaModifierV1, ()>
            + Dispatch<WpAlphaModifierSurfaceV1, AlphaModifierSurfaceUserData>
            + 'static,
    {
        let global = display.create_global::<D, WpAlphaModifierV1, _>(1, ());

        AlphaModifierState { global }
    }

    /// Returns the [WpAlphaModifierV1] global id
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// Macro to delegate implementation of the alpha modifier protocol
#[macro_export]
macro_rules! delegate_alpha_modifier {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        type __WpAlphaModifierV1 =
            $crate::reexports::wayland_protocols::wp::alpha_modifier::v1::server::wp_alpha_modifier_v1::WpAlphaModifierV1;
        type __WpAlphaModifierSurfaceV1 =
            $crate::reexports::wayland_protocols::wp::alpha_modifier::v1::server::wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1;

        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpAlphaModifierV1: ()
            ] => $crate::wayland::alpha_modifier::AlphaModifierState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpAlphaModifierV1: ()
            ] => $crate::wayland::alpha_modifier::AlphaModifierState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpAlphaModifierSurfaceV1: $crate::wayland::alpha_modifier::AlphaModifierSurfaceUserData
            ] => $crate::wayland::alpha_modifier::AlphaModifierState
        );
    };
}
