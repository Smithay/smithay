//! Utilities for handling the `wp_fractional_scale` protocol
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this implementation create the [`FractionalScaleManagerState`], store it inside your `State` struct
//! and implement the [`FractionScaleHandler`], as shown in this example:
//!
//! ```
//! use smithay::delegate_fractional_scale;
//! use smithay::wayland::compositor;
//! use smithay::reexports::wayland_server::protocol::wl_surface;
//! use smithay::wayland::fractional_scale::{
//!     self,
//!     FractionalScaleManagerState,
//!     FractionScaleHandler,
//! };
//!
//! # struct State { fractional_scale_manager_state: FractionalScaleManagerState }
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the compositor state
//! let fractional_scale_manager_state = FractionalScaleManagerState::new::<State>(
//!     &display.handle(),
//! );
//!
//! // insert the FractionalScaleManagerState into your state
//! // ..
//!
//! // implement the necessary traits
//! impl FractionScaleHandler for State {
//!    fn new_fractional_scale(&mut self, surface: wl_surface::WlSurface) {
//!        compositor::with_states(&surface, |states| {
//!            fractional_scale::with_fractional_scale(states, |fractional_scale| {
//!                // set the preferred scale for the surface
//!                fractional_scale.set_preferred_scale(1.25);
//!            });
//!        })
//!    }
//! }
//! delegate_fractional_scale!(State);
//!
//! // You're now ready to go!
//! ```
//!
//! ### Use the fractional scale state
//!
//! Whenever the fractional scale for a surface changes set the preferred
//! fractional scale like shown in the example:
//!
//! ```no_run
//! # use wayland_server::{backend::ObjectId, protocol::wl_surface, Resource};
//! use smithay::wayland::compositor;
//! use smithay::wayland::fractional_scale;
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let dh = display.handle();
//! # let surface = wl_surface::WlSurface::from_id(&dh, ObjectId::null()).unwrap();
//! compositor::with_states(&surface, |states| {
//!     fractional_scale::with_fractional_scale(states, |fractional_scale| {
//!         // set the preferred scale for the surface
//!         fractional_scale.set_preferred_scale(1.25);
//!     });
//! })
//! ```

use std::cell::RefCell;

use wayland_protocols::wp::fractional_scale::v1::server::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface, Dispatch, DisplayHandle, GlobalDispatch, Resource, Weak,
};

use super::compositor::{with_states, SurfaceData};

/// State of the wp_fractional_scale_manager_v1 Global
#[derive(Debug)]
pub struct FractionalScaleManagerState {
    global: GlobalId,
}

impl FractionalScaleManagerState {
    /// Create new [`wp_fraction_scale_manager`](wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_manager_v1) global.
    pub fn new<D>(display: &DisplayHandle) -> FractionalScaleManagerState
    where
        D: GlobalDispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()>
            + Dispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()>
            + Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, Weak<wl_surface::WlSurface>>
            + 'static,
        D: FractionScaleHandler,
    {
        FractionalScaleManagerState {
            global: display
                .create_global::<D, wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()>(1, ()),
        }
    }

    /// Returns the fractional scale manager global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, (), D>
    for FractionalScaleManagerState
where
    D: GlobalDispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()>
        + Dispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()>
        + Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, Weak<wl_surface::WlSurface>>,
    D: FractionScaleHandler,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: wayland_server::New<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
        _global_data: &(),
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, (), D>
    for FractionalScaleManagerState
where
    D: GlobalDispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()>
        + Dispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()>
        + Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, Weak<wl_surface::WlSurface>>,
    D: FractionScaleHandler,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _resource: &wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        request: <wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1 as Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wp_fractional_scale_manager_v1::Request::Destroy => {
                // All is already handled by our destructor
            }
            wp_fractional_scale_manager_v1::Request::GetFractionalScale { id, surface } => {
                let already_has_fractional_scale = with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<FractionalScaleStateUserData>()
                        .map(|v| v.borrow().is_some())
                        .unwrap_or(false)
                });

                if already_has_fractional_scale {
                    surface.post_error(
                        wp_fractional_scale_manager_v1::Error::FractionalScaleExists as u32,
                        "the surface already has a fractional_scale object associated".to_string(),
                    );
                    return;
                }

                let fractional_scale: wp_fractional_scale_v1::WpFractionalScaleV1 =
                    data_init.init(id, surface.downgrade());

                with_states(&surface, |states| {
                    if !states.data_map.insert_if_missing(|| {
                        RefCell::new(Some(FractionalScaleState {
                            fractional_scale: fractional_scale.clone(),
                            preferred_scale: None,
                        }))
                    }) {
                        let mut state = states
                            .data_map
                            .get::<FractionalScaleStateUserData>()
                            .unwrap()
                            .borrow_mut();
                        *state = Some(FractionalScaleState {
                            fractional_scale,
                            preferred_scale: None,
                        });
                    }
                });
                state.new_fractional_scale(surface);
            }
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, Weak<wl_surface::WlSurface>, D>
    for FractionalScaleManagerState
where
    D: GlobalDispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()>
        + Dispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()>
        + Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, Weak<wl_surface::WlSurface>>,
    D: FractionScaleHandler,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &wp_fractional_scale_v1::WpFractionalScaleV1,
        request: <wp_fractional_scale_v1::WpFractionalScaleV1 as Resource>::Request,
        data: &Weak<wl_surface::WlSurface>,
        _dhandle: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            wp_fractional_scale_v1::Request::Destroy => {
                if let Ok(surface) = data.upgrade() {
                    with_states(&surface, |states| {
                        states
                            .data_map
                            .get::<FractionalScaleStateUserData>()
                            .and_then(|v| v.borrow_mut().take());
                    })
                }
            }
            _ => unreachable!(),
        }
    }
}

/// Fractional scale handler type
pub trait FractionScaleHandler {
    /// A new fractional scale was instantiated
    fn new_fractional_scale(&mut self, surface: wl_surface::WlSurface);
}

/// Type stored in WlSurface states data_map
///
/// ```rs
/// compositor::with_states(surface, |states| {
///     let data = states.data_map.get::<RendererSurfaceStateUserData>();
/// });
/// ```
pub type FractionalScaleStateUserData = RefCell<Option<FractionalScaleState>>;

/// State for the fractional scale
#[derive(Debug)]
pub struct FractionalScaleState {
    fractional_scale: wp_fractional_scale_v1::WpFractionalScaleV1,
    preferred_scale: Option<f64>,
}

impl FractionalScaleState {
    /// Set the preferred scale
    pub fn set_preferred_scale(&mut self, scale: f64) {
        if self
            .preferred_scale
            .map(|preferred_scale| preferred_scale != scale)
            .unwrap_or(true)
        {
            self.fractional_scale
                .preferred_scale(f64::round(scale * 120.0) as u32);
            self.preferred_scale = Some(scale);
        }
    }

    /// Returns the current preferred scale
    pub fn preferred_scale(&self) -> Option<f64> {
        self.preferred_scale
    }
}

/// Run a closure on the [`FractionalScaleState`] of a [`WlSurface`](wl_surface::WlSurface)
///
/// Returns `None` if the surface has no fractional scale attached
pub fn with_fractional_scale<F, T>(states: &SurfaceData, f: F) -> Option<T>
where
    F: Fn(&mut FractionalScaleState) -> T,
{
    let fractional_scale = states
        .data_map
        .get::<FractionalScaleStateUserData>()
        .map(|state| state.borrow_mut());

    if let Some(mut fractional_scale) = fractional_scale {
        fractional_scale.as_mut().map(f)
    } else {
        None
    }
}

#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_fractional_scale {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1: ()
        ] => $crate::wayland::fractional_scale::FractionalScaleManagerState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1: ()
        ] => $crate::wayland::fractional_scale::FractionalScaleManagerState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::fractional_scale::v1::server::wp_fractional_scale_v1::WpFractionalScaleV1: $crate::reexports::wayland_server::Weak<$crate::reexports::wayland_server::protocol::wl_surface::WlSurface>
        ] => $crate::wayland::fractional_scale::FractionalScaleManagerState);
    };
}
