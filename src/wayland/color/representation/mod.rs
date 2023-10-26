mod protocol;
use wayland_server::{protocol::wl_surface::WlSurface, Dispatch, DisplayHandle, GlobalDispatch, Weak};

use crate::wayland::compositor::{self, Cacheable};

pub use self::protocol::*;
use self::wp_color_representation_v1::AlphaMode;
mod dispatch;

use std::sync::Mutex;

#[derive(Debug)]
pub struct ColorRepresentationState {
    coefficients: Vec<u32>,
    chroma_locations: Vec<u32>,
    known_instances: Vec<wp_color_representation_manager_v1::WpColorRepresentationManagerV1>,
}

pub trait ColorRepresentationHandler {
    fn color_representation_state(&mut self) -> &mut ColorRepresentationState;
}

#[derive(Debug, Clone, Copy, Default)]
struct ColorRepresentationSurfaceCachedState {
    coefficient: Option<u32>,
    chroma_location: Option<u32>,
    alpha_mode: Option<AlphaMode>,
}

impl Cacheable for Option<ColorRepresentationSurfaceCachedState> {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        *self
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        match (self, into) {
            (Some(this), Some(into)) => {
                into.coefficient = this.coefficient.or_else(|| into.coefficient.take());
                into.chroma_location = this.chroma_location.or_else(|| into.chroma_location.take());
                into.alpha_mode = this.alpha_mode.or_else(|| into.alpha_mode.take());
            }
            (this, into) => {
                *into = this;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ColorRepresentation {
    pub alpha_mode: AlphaMode,
    pub coefficient: Option<u32>,
    pub chroma_location: Option<u32>,
}

pub fn get_color_representation(surface: &WlSurface) -> ColorRepresentation {
    compositor::with_states(surface, |states| {
        states
            .cached_state
            .current::<Option<ColorRepresentationSurfaceCachedState>>()
            .map(|state| ColorRepresentation {
                coefficient: state.coefficient,
                chroma_location: state.chroma_location,
                alpha_mode: state.alpha_mode.unwrap_or(AlphaMode::PremultipliedElectrical),
            })
            .unwrap_or_else(|| ColorRepresentation {
                alpha_mode: AlphaMode::PremultipliedElectrical,
                coefficient: None,
                chroma_location: None,
            })
    })
}

#[derive(Debug)]
struct ColorRepresentationSurfaceData {
    instance: Mutex<Option<wp_color_representation_v1::WpColorRepresentationV1>>,
}

impl ColorRepresentationSurfaceData {
    fn new() -> Self {
        Self {
            instance: Mutex::new(None),
        }
    }

    fn is_resource_attached(&self) -> bool {
        self.instance.lock().unwrap().is_some()
    }
}

impl ColorRepresentationState {
    pub fn new<D>(
        dh: &DisplayHandle,
        coefficients: impl Iterator<Item = u32>,
        chroma_locations: impl Iterator<Item = u32>,
    ) -> ColorRepresentationState
    where
        D: GlobalDispatch<wp_color_representation_manager_v1::WpColorRepresentationManagerV1, ()>
            + Dispatch<wp_color_representation_manager_v1::WpColorRepresentationManagerV1, ()>
            + Dispatch<wp_color_representation_v1::WpColorRepresentationV1, Weak<WlSurface>>
            + ColorRepresentationHandler
            + 'static,
    {
        dh.create_global::<D, wp_color_representation_manager_v1::WpColorRepresentationManagerV1, ()>(1, ());
        ColorRepresentationState {
            coefficients: coefficients.collect(),
            chroma_locations: chroma_locations.collect(),
            known_instances: Vec::new(),
        }
    }
}

/// Macro to delegate implementation of the wp color representation protocol
#[macro_export]
macro_rules! delegate_color_representation {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        type __WpColorRepresentationManagerV1 =
            $crate::wayland::color::representation::wp_color_representation_manager_v1::WpColorRepresentationManagerV1;
        type __WpColorRepresentationV1 =
            $crate::wayland::color::representation::wp_color_representation_v1::WpColorRepresentationV1;

        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpColorRepresentationManagerV1: ()
            ] => $crate::wayland::color::representation::ColorRepresentationState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpColorRepresentationManagerV1: ()
            ] => $crate::wayland::color::representation::ColorRepresentationState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpColorRepresentationV1: $crate::reexports::wayland_server::Weak<$crate::reexports::wayland_server::protocol::wl_surface::WlSurface>
            ] => $crate::wayland::color::representation::ColorRepresentationState
        );
    };
}
