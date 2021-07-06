//! Utilities for graphics tablet support
//!
//! This module provides helpers to handle graphics tablets.
//!
//! ```
//! # extern crate wayland_server;
//! # #[macro_use] extern crate smithay;
//! # use smithay::wayland::compositor::compositor_init;
//!
//! use smithay::wayland::seat::{Seat};
//! use smithay::wayland::tablet_manager::{init_tablet_manager_global, TabletSeatTrait, TabletDescriptor};
//!
//! # let mut display = wayland_server::Display::new();
//! # compositor_init(&mut display, |_, _| {}, None);
//! // First we nee a reguler seat
//! let (seat, seat_global) = Seat::new(
//!     &mut display,
//!     "seat-0".into(),
//!     None
//! );
//!
//! // Insert the manager global into your event loop
//! init_tablet_manager_global(&mut display);
//!
//! seat
//!    .tablet_seat()                     // Get TabletSeat asosiated with this seat
//!    .add_tablet(&TabletDescriptor {    // Add a new tablet to a seat
//!        name: "Test".into(),
//!        usb_id: None,
//!        syspath: None,
//!    });
//!
//! ```

use crate::wayland::seat::Seat;
use wayland_protocols::unstable::tablet::v2::server::zwp_tablet_manager_v2::{self, ZwpTabletManagerV2};
use wayland_server::{Display, Filter, Global, Main};

const MANAGER_VERSION: u32 = 1;

mod tablet;
mod tablet_seat;
mod tablet_tool;

pub use tablet::{TabletDescriptor, TabletHandle};
pub use tablet_seat::TabletSeatHandle;
pub use tablet_tool::TabletToolHandle;

/// Extends [Seat] with graphic tablet specyfic functionality
pub trait TabletSeatTrait {
    /// Get tablet seat asosiated with this seat
    fn tablet_seat(&self) -> TabletSeatHandle;
}

impl TabletSeatTrait for Seat {
    fn tablet_seat(&self) -> TabletSeatHandle {
        let user_data = self.user_data();
        user_data.insert_if_missing(TabletSeatHandle::default);
        user_data.get::<TabletSeatHandle>().unwrap().clone()
    }
}

/// Initialize a tablet manager global.
pub fn init_tablet_manager_global(display: &mut Display) -> Global<ZwpTabletManagerV2> {
    display.create_global::<ZwpTabletManagerV2, _>(
        MANAGER_VERSION,
        Filter::new(
            move |(manager, _version): (Main<ZwpTabletManagerV2>, u32), _, _| {
                manager.quick_assign(|_manager, req, _| match req {
                    zwp_tablet_manager_v2::Request::GetTabletSeat { tablet_seat, seat } => {
                        let seat = Seat::from_resource(&seat).unwrap();

                        let user_data = seat.user_data();
                        user_data.insert_if_missing(TabletSeatHandle::default);

                        let instance = tablet_seat;
                        let tablet_seat = user_data.get::<TabletSeatHandle>().unwrap();

                        tablet_seat.add_instance(instance);
                    }
                    zwp_tablet_manager_v2::Request::Destroy => {
                        // Nothing to do
                    }
                    _ => {}
                });
            },
        ),
    )
}
