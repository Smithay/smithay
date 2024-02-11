//! Utilities for handling the `wp_fractional_scale` protocol
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this implementation create the [`FractionalScaleManagerState`], store it inside your `State` struct
//! and implement the [`FractionalScaleHandler`], as shown in this example:
//!
//! ```
//! use smithay::wayland::compositor;
//! use smithay::reexports::wayland_server::protocol::wl_surface;
//! use smithay::wayland::fractional_scale::{
//!     self,
//!     FractionalScaleManagerState,
//!     FractionalScaleHandler,
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
//! impl FractionalScaleHandler for State {
//!    fn new_fractional_scale(&mut self, surface: wl_surface::WlSurface) {
//!        compositor::with_states(&surface, |states| {
//!            fractional_scale::with_fractional_scale(states, |fractional_scale| {
//!                // set the preferred scale for the surface
//!                fractional_scale.set_preferred_scale(1.25);
//!            });
//!        })
//!    }
//! }
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
    wp_fractional_scale_manager_v1::{self, WpFractionalScaleManagerV1},
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Dispatch, DisplayHandle, GlobalDispatch, Resource,
    Weak,
};

use super::compositor::{with_states, SurfaceData};

/// State of the wp_fractional_scale_manager_v1 Global
#[derive(Debug)]
pub struct FractionalScaleManagerState {
    global: GlobalId,
}

impl FractionalScaleManagerState {
    /// Create new [`WpFractionalScaleManagerV1`] global.
    pub fn new<D>(display: &DisplayHandle) -> FractionalScaleManagerState
    where
        D: FractionalScaleHandler,
    {
        FractionalScaleManagerState {
            global: display.create_delegated_global::<D, WpFractionalScaleManagerV1, (), Self>(1, ()),
        }
    }

    /// Returns the fractional scale manager global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<WpFractionalScaleManagerV1, (), D> for FractionalScaleManagerState
where
    D: FractionalScaleHandler,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &wayland_server::Client,
        resource: wayland_server::New<WpFractionalScaleManagerV1>,
        _global_data: &(),
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        data_init.init_delegated::<_, _, Self>(resource, ());
    }
}

impl<D> Dispatch<WpFractionalScaleManagerV1, (), D> for FractionalScaleManagerState
where
    D: FractionalScaleHandler,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WpFractionalScaleManagerV1,
        request: wp_fractional_scale_manager_v1::Request,
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
                    data_init.init_delegated::<_, _, Self>(id, surface.downgrade());

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

impl<D> Dispatch<WpFractionalScaleV1, Weak<WlSurface>, D> for FractionalScaleManagerState {
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _resource: &WpFractionalScaleV1,
        request: wp_fractional_scale_v1::Request,
        data: &Weak<WlSurface>,
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
pub trait FractionalScaleHandler: 'static {
    /// A new fractional scale was instantiated
    fn new_fractional_scale(&mut self, surface: WlSurface);
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

#[deprecated(note = "No longer needed, this is now NOP")]
#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_fractional_scale {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {};
}
