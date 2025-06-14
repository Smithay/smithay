//! Implementation of `ext_background_effect_manager_v1` protocol

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use crate::wayland::compositor::Cacheable;
use wayland_protocols::ext::background_effect::v1::server::{
    ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1,
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Dispatch, DisplayHandle, GlobalDispatch, Resource,
    Weak,
};

mod dispatch;

/// Cached state for background effect per surface (double-buffered)
#[derive(Debug, Clone, Default)]
pub struct BackgroundEffectSurfaceCachedState {
    pub blur_region: Option<crate::wayland::compositor::RegionAttributes>,
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
    pub fn new<D>(display: &DisplayHandle) -> BackgroundEffectState
    where
        D: GlobalDispatch<ExtBackgroundEffectManagerV1, ()>
            + Dispatch<ExtBackgroundEffectManagerV1, ()>
            + Dispatch<ExtBackgroundEffectSurfaceV1, BackgroundEffectSurfaceUserData>
            + 'static,
    {
        let global = display.create_global::<D, ExtBackgroundEffectManagerV1, _>(1, ());
        BackgroundEffectState { global }
    }

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
