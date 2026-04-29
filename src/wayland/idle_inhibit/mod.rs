//! Utilities for handling the `zwp_idle_inhibit` protocol
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! To initialize this implementation create the [`IdleInhibitManagerState`] and
//! implement the [`IdleInhibitHandler`], as shown in this example:
//!
//! ```
//! use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
//! use smithay::wayland::idle_inhibit::{IdleInhibitManagerState, IdleInhibitHandler};
//!
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! // Create the compositor state
//! IdleInhibitManagerState::new::<State>(&display.handle());
//!
//! // implement the necessary trait
//! impl IdleInhibitHandler for State {
//!    fn inhibit(&mut self, surface: WlSurface) {
//!        // …
//!    }
//!
//!    fn uninhibit(&mut self, surface: WlSurface) {
//!        // …
//!    }
//! }
//!
//! smithay::delegate_dispatch2!(State);
//!
//! // You're now ready to go!
//! ```

use _idle_inhibit::zwp_idle_inhibit_manager_v1::{Request, ZwpIdleInhibitManagerV1};
use _idle_inhibit::zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1;
use wayland_protocols::wp::idle_inhibit::zv1::server as _idle_inhibit;
use wayland_server::backend::GlobalId;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};

use crate::wayland::GlobalData;
use crate::wayland::idle_inhibit::inhibitor::IdleInhibitorState;

pub mod inhibitor;

const MANAGER_VERSION: u32 = 1;

/// State of the zwp_idle_inhibit_manager_v1 Global
#[derive(Debug)]
pub struct IdleInhibitManagerState {
    global: GlobalId,
}

impl IdleInhibitManagerState {
    /// Create new [`zwp_idle_inhibit_manager`](ZwpIdleInhibitManagerV1) global.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: IdleInhibitHandler,
        D: 'static,
    {
        let global = display.create_global::<D, ZwpIdleInhibitManagerV1, _>(MANAGER_VERSION, GlobalData);

        Self { global }
    }

    /// Returns the fractional scale manager global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<ZwpIdleInhibitManagerV1, D> for GlobalData
where
    D: IdleInhibitHandler,
    D: 'static,
{
    fn bind(
        &self,
        _state: &mut D,
        _display: &DisplayHandle,
        _client: &Client,
        manager: New<ZwpIdleInhibitManagerV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, GlobalData);
    }
}

impl<D> Dispatch<ZwpIdleInhibitManagerV1, D> for GlobalData
where
    D: IdleInhibitHandler,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        _manager: &ZwpIdleInhibitManagerV1,
        request: Request,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            Request::CreateInhibitor { id, surface } => {
                state.inhibit(surface.clone());
                data_init.init(id, IdleInhibitorState::new(surface));
            }
            Request::Destroy => (),
            _ => unreachable!(),
        }
    }
}

/// Handler trait for idle-inhibit.
pub trait IdleInhibitHandler {
    /// Enable idle inhibition for the output of the provided surface.
    fn inhibit(&mut self, surface: WlSurface);

    /// Stop inhibition for the provided surface.
    ///
    /// This function is only called when a client explicitly removes the session
    /// inhibition. It is up to the compositor to ignore inhibiting surfaces which
    /// are invisible or dead.
    fn uninhibit(&mut self, surface: WlSurface);
}
