//! XDG System Bell
//!
//! This protocol enables clients to ring the system bell.
//!
//! In order to advertise system bell global call [`XdgSystemBellState::new`].
//!
//! ```
//! use smithay::wayland::xdg_system_bell::{XdgSystemBellState, XdgSystemBellHandler};
//! use wayland_server::protocol::wl_surface::WlSurface;
//!
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//!
//! XdgSystemBellState::new::<State>(
//!     &display.handle(),
//! );
//!
//! // provide the necessary trait implementations
//! impl XdgSystemBellHandler for State {
//!     fn ring(&mut self, surface: Option<WlSurface>) {
//!         println!("Ring got called");
//!     }
//! }
//!
//! smithay::delegate_dispatch2!(State);
//! ```

use wayland_protocols::xdg::system_bell::v1::server::xdg_system_bell_v1::{self, XdgSystemBellV1};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, backend::GlobalId,
    protocol::wl_surface::WlSurface,
};

use crate::wayland::GlobalData;

/// Handler for xdg ring request
pub trait XdgSystemBellHandler: 'static {
    /// Ring the system bell
    fn ring(&mut self, surface: Option<WlSurface>);
}

/// State of the xdg system bell
#[derive(Debug)]
pub struct XdgSystemBellState {
    global_id: GlobalId,
}

impl XdgSystemBellState {
    /// Register new [XdgSystemBellV1] global
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: XdgSystemBellHandler,
    {
        let global_id = display.create_global::<D, XdgSystemBellV1, _>(1, GlobalData);
        Self { global_id }
    }

    /// [XdgSystemBellV1] GlobalId getter
    pub fn global(&self) -> GlobalId {
        self.global_id.clone()
    }
}

impl<D: XdgSystemBellHandler> GlobalDispatch<XdgSystemBellV1, D> for GlobalData {
    fn bind(
        &self,
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<XdgSystemBellV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }
}

impl<D: XdgSystemBellHandler> Dispatch<XdgSystemBellV1, D> for GlobalData {
    fn request(
        &self,
        state: &mut D,
        _client: &wayland_server::Client,
        _resource: &XdgSystemBellV1,
        request: xdg_system_bell_v1::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_system_bell_v1::Request::Ring { surface } => {
                state.ring(surface);
            }
            xdg_system_bell_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}
