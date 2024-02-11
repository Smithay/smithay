//! Utilities for graphics tablet support
//!
//! This module provides helpers to handle graphics tablets.
//!
//! ```
//! use smithay::input::{Seat, SeatState, SeatHandler, pointer::CursorImageStatus};
//! use smithay::wayland::tablet_manager::{TabletManagerState, TabletDescriptor};
//! use smithay::reexports::wayland_server::{Display, protocol::wl_surface::WlSurface};
//!
//! # struct State { seat_state: SeatState<Self> };
//! # let mut display = Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//!
//! let mut seat_state = SeatState::<State>::new();
//! let tablet_state = TabletManagerState::new::<State>(&display_handle);
//! // add the seat state to your state
//! // ...
//!
//! // create the seat
//! let seat = seat_state.new_wl_seat(
//!     &display_handle,          // the display
//!     "seat-0",                 // the name of the seat, will be advertized to clients
//! );
//!
//! use smithay::wayland::tablet_manager::TabletSeatTrait;
//!
//! seat
//!    .tablet_seat()                     // Get TabletSeat asosiated with this seat
//!    .add_tablet::<State>(              // Add a new tablet to a seat
//!      &display_handle,
//!      &TabletDescriptor {    
//!        name: "Test".into(),
//!        usb_id: None,
//!        syspath: None,
//!      }
//!    );
//!
//! // implement the required traits
//! impl SeatHandler for State {
//!     type KeyboardFocus = WlSurface;
//!     type PointerFocus = WlSurface;
//!     type TouchFocus = WlSurface;
//!     fn seat_state(&mut self) -> &mut SeatState<Self> {
//!         &mut self.seat_state
//!     }
//!     fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
//!         // ...
//!     }
//!     fn cursor_image(&mut self, seat: &Seat<Self>, image: CursorImageStatus) {
//!         // ...
//!     }
//! }
//! ```
//! ```ignore
//! // Init the manager global
//! let state = TabletManagerState::new::<D>(&display);
//!
//! // Init the seat
//! let seat = Seat::<D>::new(
//!     &display,
//!     "seat-0".into(),
//!     None
//! );
//!
//! use smithay::wyaldnd::tablet_manager::TabletSeatTrait;
//!
//! seat
//!    .tablet_seat()                     // Get TabletSeat asosiated with this seat
//!    .add_tablet(                       // Add a new tablet to a seat
//!      display
//!      &TabletDescriptor {    
//!        name: "Test".into(),
//!        usb_id: None,
//!        syspath: None,
//!      }
//!    );
//! ```

use crate::input::{Seat, SeatHandler};
use wayland_protocols::wp::tablet::zv2::server::zwp_tablet_manager_v2::{self, ZwpTabletManagerV2};
use wayland_server::{backend::GlobalId, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};

const MANAGER_VERSION: u32 = 1;

mod tablet;
mod tablet_seat;
pub(crate) mod tablet_tool;

pub use tablet::{TabletDescriptor, TabletHandle, TabletUserData};
pub use tablet_seat::{TabletSeatHandle, TabletSeatUserData};
pub use tablet_tool::{TabletToolHandle, TabletToolUserData};

/// Extends [Seat] with graphic tablet specific functionality
pub trait TabletSeatTrait {
    /// Get tablet seat associated with this seat
    fn tablet_seat(&self) -> TabletSeatHandle;
}

impl<D: SeatHandler + 'static> TabletSeatTrait for Seat<D> {
    fn tablet_seat(&self) -> TabletSeatHandle {
        let user_data = self.user_data();
        user_data.insert_if_missing(TabletSeatHandle::default);
        user_data.get::<TabletSeatHandle>().unwrap().clone()
    }
}

/// State of wp tablet protocol
#[derive(Debug)]
pub struct TabletManagerState {
    global: GlobalId,
}

impl TabletManagerState {
    /// Initialize a tablet manager global.
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: SeatHandler + 'static,
    {
        let global = display.create_delegated_global::<D, ZwpTabletManagerV2, _, Self>(MANAGER_VERSION, ());

        Self { global }
    }

    /// Get the id of ZwpTabletManagerV2 global
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

impl<D> GlobalDispatch<ZwpTabletManagerV2, (), D> for TabletManagerState
where
    D: SeatHandler + 'static,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpTabletManagerV2>,
        _: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init_delegated::<_, _, Self>(resource, ());
    }
}

impl<D> Dispatch<ZwpTabletManagerV2, (), D> for TabletManagerState
where
    D: SeatHandler + 'static,
{
    fn request(
        _state: &mut D,
        client: &Client,
        _: &ZwpTabletManagerV2,
        request: zwp_tablet_manager_v2::Request,
        _: &(),
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            zwp_tablet_manager_v2::Request::GetTabletSeat { tablet_seat, seat } => {
                let seat = Seat::<D>::from_resource(&seat).unwrap();

                let user_data = seat.user_data();
                user_data.insert_if_missing(TabletSeatHandle::default);

                let handle = user_data.get::<TabletSeatHandle>().unwrap();
                let instance = data_init.init_delegated::<_, _, Self>(
                    tablet_seat,
                    TabletSeatUserData {
                        handle: handle.clone(),
                    },
                );

                handle.add_instance::<D>(dh, &instance, client);
            }
            zwp_tablet_manager_v2::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }
}

#[deprecated(note = "No longer needed, this is now NOP")]
#[allow(missing_docs)] // TODO
#[macro_export]
macro_rules! delegate_tablet_manager {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {};
}
