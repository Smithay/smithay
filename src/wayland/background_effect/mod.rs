//! Implementation of `ext_background_effect_manager_v1` protocol
//!
//! In order to advertise background effect global call [BackgroundEffectState::new] and delegate
//! events to it with [`delegate_background_effect`][crate::delegate_background_effect].
//! Currently attached effects are available in double-buffered [BackgroundEffectSurfaceCachedState]
//!
//! ```
//! use smithay::wayland::background_effect::{BackgroundEffectState, ExtBackgroundEffectHandler, Capability};
//! use wayland_server::protocol::wl_surface::WlSurface;
//! use smithay::delegate_background_effect;
//! use smithay::wayland::compositor;
//!
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! BackgroundEffectState::new::<State>(
//!     &display.handle(),
//! );
//!
//! impl ExtBackgroundEffectHandler for State {
//!     fn capabilities(&self) -> Capability {
//!         Capability::Blur
//!     }
//!
//!     fn set_blur_region(&mut self, wl_surface: WlSurface, region: compositor::RegionAttributes) {
//!         // Called when blur becomes pending, and awaits surface commit.
//!         // Blur region is stored in wl_surface [BackgroundEffectSurfaceCachedState]
//!     }
//! }
//!
//! delegate_background_effect!(State);
//! ```

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use crate::wayland::compositor::{self, Cacheable};
use wayland_protocols::ext::background_effect::v1::server::{
    ext_background_effect_manager_v1::{self, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Dispatch, DisplayHandle, GlobalDispatch, Resource,
    Weak,
};

pub use ext_background_effect_manager_v1::Capability;

mod dispatch;

/// Handler trait for ext background effect events.
pub trait ExtBackgroundEffectHandler:
    GlobalDispatch<ExtBackgroundEffectManagerV1, ()>
    + Dispatch<ExtBackgroundEffectManagerV1, ()>
    + Dispatch<ExtBackgroundEffectSurfaceV1, BackgroundEffectSurfaceUserData>
    + 'static
{
    /// Getter for background-effect capabilities of the compositor
    fn capabilities(&self) -> Capability {
        // For now only blur is supported, so it's safe to assume it's supported by default
        Capability::Blur
    }

    /// Called when blur becomes pending, and awaits surface commit.
    /// Blur region is stored in wl_surface [BackgroundEffectSurfaceCachedState]
    fn set_blur_region(&mut self, wl_surface: WlSurface, region: compositor::RegionAttributes) {
        let _ = wl_surface;
        let _ = region;
    }
}

/// Cached state for background effect per surface (double-buffered)
///
/// ```no_run
/// use smithay::wayland::compositor;
/// use smithay::wayland::background_effect::BackgroundEffectSurfaceCachedState;
///
/// # let wl_surface = todo!();
/// compositor::with_states(&wl_surface, |states| {
///     let mut modifier_state = states.cached_state.get::<BackgroundEffectSurfaceCachedState>();
///     dbg!(&modifier_state.current().blur_region);
/// });
/// ```
#[derive(Debug, Clone, Default)]
pub struct BackgroundEffectSurfaceCachedState {
    /// Region of the surface that will have its background blurred.
    pub blur_region: Option<compositor::RegionAttributes>,
}

impl Cacheable for BackgroundEffectSurfaceCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        self.clone()
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

/// User data for ext_background_effect_surface_v1
#[derive(Debug)]
pub struct BackgroundEffectSurfaceUserData(Mutex<Weak<WlSurface>>);

impl BackgroundEffectSurfaceUserData {
    fn new(surface: WlSurface) -> Self {
        Self(Mutex::new(surface.downgrade()))
    }

    fn wl_surface(&self) -> Option<WlSurface> {
        self.0.lock().unwrap().upgrade().ok()
    }
}

/// Tracks if a surface already has a background effect object
#[derive(Debug)]
struct BackgroundEffectSurfaceData {
    is_resource_attached: AtomicBool,
}

impl BackgroundEffectSurfaceData {
    fn new() -> Self {
        Self {
            is_resource_attached: AtomicBool::new(false),
        }
    }

    fn set_is_resource_attached(&self, is_attached: bool) {
        self.is_resource_attached.store(is_attached, Ordering::Release)
    }

    fn is_resource_attached(&self) -> bool {
        self.is_resource_attached.load(Ordering::Acquire)
    }
}

/// Global state for background effect protocol
#[derive(Debug)]
pub struct BackgroundEffectState {
    global: GlobalId,
}

impl BackgroundEffectState {
    /// Regiseter new [ExtBackgroundEffectManagerV1] global
    pub fn new<D: ExtBackgroundEffectHandler>(display: &DisplayHandle) -> BackgroundEffectState {
        let global = display.create_global::<D, ExtBackgroundEffectManagerV1, _>(1, ());
        BackgroundEffectState { global }
    }

    /// Returns the [ExtBackgroundEffectManagerV1] global id
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// Macro to delegate implementation of the background effect protocol
#[macro_export]
macro_rules! delegate_background_effect {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        type __ExtBackgroundEffectManagerV1 =
            $crate::reexports::wayland_protocols::ext::background_effect::v1::server::ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1;
        type __ExtBackgroundEffectSurfaceV1 =
            $crate::reexports::wayland_protocols::ext::background_effect::v1::server::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1;

        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __ExtBackgroundEffectManagerV1: ()
            ] => $crate::wayland::background_effect::BackgroundEffectState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __ExtBackgroundEffectManagerV1: ()
            ] => $crate::wayland::background_effect::BackgroundEffectState
        );
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __ExtBackgroundEffectSurfaceV1: $crate::wayland::background_effect::BackgroundEffectSurfaceUserData
            ] => $crate::wayland::background_effect::BackgroundEffectState
        );
    }
}
