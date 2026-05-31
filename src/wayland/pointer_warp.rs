//! Wp Pointer Warp
//!
//! This global interface allows applications to request the pointer to be moved to a position
//! relative to a wl_surface.
//!
//! In order to advertise pointer warp global call [PointerWarpManager::new].
//!
//! ```
//! use smithay::wayland::pointer_warp::{PointerWarpManager, PointerWarpHandler};
//! use wayland_server::protocol::wl_surface::WlSurface;
//! use wayland_server::protocol::wl_pointer::WlPointer;
//! use smithay::utils::{Serial, Logical, Point};
//!
//! # struct State;
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! #
//! # use smithay::wayland::compositor::{CompositorHandler, CompositorState, CompositorClientState};
//! # use smithay::reexports::wayland_server::Client;
//! # impl CompositorHandler for State {
//! #     fn compositor_state(&mut self) -> &mut CompositorState { unimplemented!() }
//! #     fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState { unimplemented!() }
//! #     fn commit(&mut self, surface: &WlSurface) {}
//! # }
//! #
//! # impl smithay::input::SeatHandler for State {
//! #     type KeyboardFocus = WlSurface;
//! #     type PointerFocus = WlSurface;
//! #     type TouchFocus = WlSurface;
//! #     fn seat_state(&mut self) -> &mut smithay::input::SeatState<Self> {
//! #         todo!()
//! #     }
//! # }
//!
//! PointerWarpManager::new::<State>(
//!     &display.handle(),
//! );
//!
//! impl PointerWarpHandler for State {
//!     fn warp_pointer(&mut self, surface: WlSurface, pointer: WlPointer, pos: Point<f64, Logical>, serial: Serial) {
//!         // Pointer warp was requested by the client
//!     }
//! }
//!
//! smithay::delegate_dispatch2!(State);
//! ```

use std::sync::atomic::Ordering;

use wayland_protocols::wp::pointer_warp::v1::server::wp_pointer_warp_v1::{self, WpPointerWarpV1};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
    backend::GlobalId,
    protocol::{wl_pointer::WlPointer, wl_surface::WlSurface},
};

use crate::{
    input::SeatHandler,
    utils::{Client as ClientCords, Logical, Point, Serial},
    wayland::{Dispatch2, GlobalData, GlobalDispatch2, seat::PointerUserData},
};

/// Handler trait for pointer warp events.
pub trait PointerWarpHandler: SeatHandler + 'static {
    /// Request the compositor to move the pointer to a surface-local position.
    /// Whether or not the compositor honors the request is implementation defined, but it should
    /// - honor it if the surface has pointer focus, including when it has an implicit pointer grab
    /// - reject it if the enter serial is incorrect
    /// - reject it if the requested position is outside of the surface
    ///
    /// Note that the enter serial is valid for any surface of the client, and does not have to be from the surface the pointer is warped to.
    ///
    /// * `serial` - serial number of the surface enter event
    #[allow(unused)]
    fn warp_pointer(
        &mut self,
        surface: WlSurface,
        pointer: WlPointer,
        pos: Point<f64, Logical>,
        serial: Serial,
    ) {
    }
}

/// Delegate type for handling pointer warp events.
#[derive(Debug)]
pub struct PointerWarpManager {
    global: GlobalId,
}

impl PointerWarpManager {
    /// Creates a new delegate type for handling [WpPointerWarpV1] events.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: PointerWarpHandler + GlobalDispatch<WpPointerWarpV1, GlobalData>,
    {
        let global = display.create_global::<D, WpPointerWarpV1, _>(1, GlobalData);
        Self { global }
    }

    /// Returns the [WpPointerWarpV1] global id.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D: PointerWarpHandler> GlobalDispatch2<WpPointerWarpV1, D> for GlobalData
where
    D: Dispatch<WpPointerWarpV1, GlobalData>,
{
    fn bind(
        &self,
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WpPointerWarpV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }
}

impl<D: PointerWarpHandler> Dispatch2<WpPointerWarpV1, D> for GlobalData {
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        _resource: &WpPointerWarpV1,
        request: wp_pointer_warp_v1::Request,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        use wp_pointer_warp_v1::Request;

        match request {
            Request::WarpPointer {
                surface,
                pointer,
                x,
                y,
                serial,
            } => {
                let client_scale = pointer
                    .data::<PointerUserData<D>>()
                    .unwrap()
                    .client_scale
                    .load(Ordering::Acquire);
                let location: Point<f64, ClientCords> = Point::new(x, y);
                let location = location.to_logical(client_scale);

                state.warp_pointer(surface, pointer, location, Serial::from(serial));
            }
            Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}
