//! Implementation of `ext_background_effect_manager_v1` protocol
//!
//! In order to advertise background effect global call [BackgroundEffectState::new].
//! Currently attached effects are available in double-buffered [BackgroundEffectSurfaceCachedState]
//!
//! ```
//! use smithay::wayland::background_effect::{BackgroundEffectState, ExtBackgroundEffectHandler, Capability};
//! use wayland_server::protocol::wl_surface::WlSurface;
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
//! smithay::delegate_dispatch2!(State);
//! ```

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use crate::wayland::{
    GlobalData,
    compositor::{self, Cacheable},
};
use wayland_protocols::ext::background_effect::v1::server::ext_background_effect_manager_v1::{
    self, ExtBackgroundEffectManagerV1,
};
use wayland_server::{
    DisplayHandle, GlobalDispatch, Resource, Weak, backend::GlobalId, protocol::wl_surface::WlSurface,
};

pub use ext_background_effect_manager_v1::Capability;

mod dispatch;

/// Handler trait for ext background effect events.
pub trait ExtBackgroundEffectHandler: 'static {
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

    /// Called when blur unset becomes pending, and awaits surface commit.
    fn unset_blur_region(&mut self, wl_surface: WlSurface) {
        let _ = wl_surface;
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
    pub fn new<D>(display: &DisplayHandle) -> BackgroundEffectState
    where
        D: ExtBackgroundEffectHandler + GlobalDispatch<ExtBackgroundEffectManagerV1, GlobalData>,
    {
        let global = display.create_global::<D, ExtBackgroundEffectManagerV1, _>(1, GlobalData);
        BackgroundEffectState { global }
    }

    /// Returns the [ExtBackgroundEffectManagerV1] global id
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}
