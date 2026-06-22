//! Utilities for graphics tablet support
//!
//! This module provides you with utilities for handling the tablet manager globals and the
//! associated Wayland objects.
//!
//! ## How to use it
//!
//! ### Initialization
//!
//! ```
//! use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
//! use smithay::input::tablet::{TabletSeatHandler, TabletSeatTrait};
//! use smithay::backend::input::TabletToolDescriptor;
//! use smithay::wayland::tablet_manager::{TabletManagerState};
//! use smithay::reexports::wayland_server::{Display, protocol::wl_surface::WlSurface};
//! # use smithay::wayland::compositor::{CompositorHandler, CompositorState, CompositorClientState};
//! # use smithay::reexports::wayland_server::Client;
//!
//! # struct State { seat_state: SeatState<Self> };
//! # let mut display = Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! let mut seat_state = SeatState::<State>::new();
//! let tablet_state = TabletManagerState::new::<State>(&display_handle);
//! // add the seat state and tablet manager state to your state.
//! // ...
//!
//! // create the seat
//! let seat = seat_state.new_wl_seat(
//!     &display_handle,            // the display
//!     "seat-0",                   // the name of the seat, will be advertised to clients
//! );
//! // create the associated tablet seat.
//! let tablet_seat = seat.tablet_seat();
//!
//! // implement the required traits
//! impl SeatHandler for State {
//!     type KeyboardFocus = WlSurface;
//!     type PointerFocus = WlSurface;
//!     type TouchFocus = WlSurface;
//!
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//!
//!     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
//!         // handle focus changes, if you need to ...
//!     }
//!     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) {
//!         // handle new images for the cursor ...
//!     }
//! }
//!
//! impl TabletSeatHandler for State {
//!     type ToolFocus = WlSurface;
//!
//!     fn tablet_tool_image(&mut self, tool: &TabletToolDescriptor, image: CursorImageStatus) {
//!         // handle new image for the given tool.
//!     }
//! }
//!
//! smithay::delegate_dispatch2!(State);
//!
//! # impl CompositorHandler for State {
//! #     fn compositor_state(&mut self) -> &mut CompositorState { unimplemented!() }
//! #     fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState { unimplemented!() }
//! #     fn commit(&mut self, surface: &WlSurface) {}
//! # }
//! ```
//!
//! ### Run usage
//!
//! Once the seat is initialized, you can add tablet and tools to it.
//!
//! You can add these via methods of the [`TabletSeat`] struct:
//! [`TabletSeat::add_wp_tablet`] and [`TabletSeat::add_wp_tool`]. These methods return the same
//! handle their non-wayland counterpart do, but additionally expose ZwpTablet* objects to wayland
//! clients.

use crate::{
    input::{
        Seat, SeatHandler,
        tablet::{TabletSeat, TabletSeatHandler},
    },
    wayland::{Dispatch2, GlobalData, GlobalDispatch2, compositor::CompositorHandler},
};
use wayland_protocols::wp::tablet::zv2::server::{
    zwp_tablet_manager_v2::{self, ZwpTabletManagerV2},
    zwp_tablet_seat_v2::ZwpTabletSeatV2,
    zwp_tablet_tool_v2::ZwpTabletToolV2,
    zwp_tablet_v2::ZwpTabletV2,
};

use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, backend::GlobalId};
const MANAGER_VERSION: u32 = 1;

pub(crate) mod tablet;
mod tablet_seat;
pub(crate) mod tablet_tool;

pub use tablet::TabletUserData;
pub use tablet_seat::TabletSeatUserData;
pub use tablet_tool::TabletToolUserData;

/// State of wp tablet protocol
#[derive(Debug)]
pub struct TabletManagerState {
    global: GlobalId,
}

impl TabletManagerState {
    /// Initialize a tablet manager global.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwpTabletManagerV2, GlobalData>,
        D: Dispatch<ZwpTabletManagerV2, GlobalData>,
        D: Dispatch<ZwpTabletSeatV2, TabletSeatUserData<D>>,
        D: Dispatch<ZwpTabletToolV2, TabletToolUserData<D>>,
        D: TabletSeatHandler,
        D: 'static,
    {
        let global = display.create_global::<D, ZwpTabletManagerV2, _>(MANAGER_VERSION, GlobalData);

        Self { global }
    }

    /// Get the id of ZwpTabletManagerV2 global
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch2<ZwpTabletManagerV2, D> for GlobalData
where
    D: Dispatch<ZwpTabletManagerV2, GlobalData>,
    D: Dispatch<ZwpTabletSeatV2, TabletSeatUserData<D>>,
    D: TabletSeatHandler,
{
    fn bind(
        &self,
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpTabletManagerV2>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }
}

impl<D> Dispatch2<ZwpTabletManagerV2, D> for GlobalData
where
    D: Dispatch<ZwpTabletSeatV2, TabletSeatUserData<D>>,
    D: Dispatch<ZwpTabletV2, TabletUserData>,
    D: Dispatch<ZwpTabletToolV2, TabletToolUserData<D>>,
    D: SeatHandler + TabletSeatHandler + 'static,
    D: CompositorHandler,
{
    fn request(
        &self,
        state: &mut D,
        client: &Client,
        _resource: &ZwpTabletManagerV2,
        request: <ZwpTabletManagerV2 as wayland_server::Resource>::Request,
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_tablet_manager_v2::Request::GetTabletSeat { tablet_seat, seat } => {
                let seat = Seat::<D>::from_resource(&seat).unwrap();

                let user_data = seat.user_data();
                user_data.insert_if_missing(TabletSeat::<D>::default);

                let handle = user_data.get::<TabletSeat<D>>().unwrap();
                let instance = data_init.init(
                    tablet_seat,
                    TabletSeatUserData {
                        handle: handle.clone(),
                    },
                );

                handle.add_instance(state, dh, &instance, client);
            }
            zwp_tablet_manager_v2::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(
        &self,
        _state: &mut D,
        _client: wayland_server::backend::ClientId,
        _resource: &ZwpTabletManagerV2,
    ) {
    }
}
