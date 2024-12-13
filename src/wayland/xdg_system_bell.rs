//! XDG System Bell
//!
//! This protocol enables clients to ring the system bell.
//!
//! In order to advertise system bell global call [`XdgSystemBellState::new`] and delegate
//! events to it with [`delegate_xdg_system_bell`][crate::delegate_xdg_system_bell].
//!
//! ```
//! use smithay::wayland::xdg_system_bell::{XdgSystemBellState, XdgSystemBellHandler};
//! use wayland_server::protocol::wl_surface::WlSurface;
//! use smithay::delegate_xdg_system_bell;
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
//! delegate_xdg_system_bell!(State);
//! ```

use wayland_protocols::xdg::system_bell::v1::server::xdg_system_bell_v1::{self, XdgSystemBellV1};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle,
    GlobalDispatch, New,
};

/// Handler for xdg ring request
pub trait XdgSystemBellHandler:
    GlobalDispatch<XdgSystemBellV1, ()> + Dispatch<XdgSystemBellV1, ()> + 'static
{
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
    pub fn new<D: XdgSystemBellHandler>(display: &DisplayHandle) -> Self {
        let global_id = display.create_global::<D, XdgSystemBellV1, ()>(1, ());
        Self { global_id }
    }

    /// [XdgSystemBellV1] GlobalId getter
    pub fn global(&self) -> GlobalId {
        self.global_id.clone()
    }
}

impl<D: XdgSystemBellHandler> GlobalDispatch<XdgSystemBellV1, (), D> for XdgSystemBellState {
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<XdgSystemBellV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D: XdgSystemBellHandler> Dispatch<XdgSystemBellV1, (), D> for XdgSystemBellState {
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        _resource: &XdgSystemBellV1,
        request: xdg_system_bell_v1::Request,
        _data: &(),
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

/// Macro to delegate implementation of the xdg system bell to [`XdgSystemBellState`].
///
/// You must also implement [`XdgSystemBellHandler`] to use this.
#[macro_export]
macro_rules! delegate_xdg_system_bell {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::system_bell::v1::server::xdg_system_bell_v1::XdgSystemBellV1: ()
        ] => $crate::wayland::xdg_system_bell::XdgSystemBellState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::xdg::system_bell::v1::server::xdg_system_bell_v1::XdgSystemBellV1: ()
        ] => $crate::wayland::xdg_system_bell::XdgSystemBellState);
    };
}
