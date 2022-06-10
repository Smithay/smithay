//! Protocol-related utilities
//!
//! This module contains several handlers to manage the Wayland protocol
//! and the clients.
//!
//! ## General structure
//!
//! Most utilities provided in this module work in the same way:
//!
//! - A module specific `*State` struct will take the wayland display as argument and
//!   insert one or more globals into it through its constructor.
//! - The module-`State` will have to be stored inside your global compositor state.
//!   (The same type you parametrized [`wayland_server::Display`] over.)
//! - You need to implement a module-specific `*Handler`-trait for your compositor state.
//!   This implementation will be called when wayland events require custom handling.
//! - Call the matching `delegate_*!` macro from smithay on your state to implement
//!   some required `wayland_server` traits.
//! - If you want to remove a previously inserted global, just drop the `*State`.
//!
//! ## Provided helpers
//!
//! ### Core functionality
//!
//! The most fundamental module is the [`compositor`] module, which provides the necessary
//! logic to handle the fundamental component by which clients build their windows: surfaces.
//! Following this, the [`shell`] module contains the logic allowing clients to use their
//! surface to build concrete windows with the usual interactions. Different kind of shells
//! exist, but in general you will want to support at least the [`xdg`](shell::xdg) variant,
//! which is the standard used by most applications.
//!
//! Then, the [`seat`] module contains logic related to input handling. These helpers are used
//! to forward input (such as pointer action or keystrokes) to clients, and manage the input
//! focus of clients. Tightly coupled with it is the [`data_device`] module, which handles
//! cross-client interactions such as accessing the clipboard, or drag'n'drop actions.
//!
//! The [`shm`] module provides the necessary logic for client to provide buffers defining the
//! contents of their windows using shared memory. This is the main mechanism used by clients
//! that are not hardware accelerated. As a complement, the [`dmabuf`] module provides support
//! hardware-accelerated clients; it is tightly linked to the
//! [`backend::allocator`](crate::backend::allocator) module.
//!
//! The [`output`] module helps forwarding to clients information about the display monitors that
//! are available. This notably plays a key role in HiDPI handling, and more generally notifying
//! clients about whether they are currently visible or not (allowing them to stop drawing if they
//! are not, for example).
//!

use std::sync::atomic::{AtomicU32, Ordering};

pub mod buffer;
pub mod compositor;
pub mod data_device;
pub mod dmabuf;
pub mod output;
pub mod seat;
pub mod shell;
pub mod shm;
pub mod socket;
pub mod tablet_manager;
pub mod xdg_activation;

/// A global [`SerialCounter`] for use in your compositor.
///
/// Is is also used internally by some parts of Smithay.
pub static SERIAL_COUNTER: SerialCounter = SerialCounter {
    serial: AtomicU32::new(0),
};

/// A serial type, whose comparison takes into account the wrapping-around behavior of the
/// underlying counter.
#[derive(Debug, Copy, Clone)]
pub struct Serial(u32);

impl PartialEq for Serial {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Serial {}

impl PartialOrd for Serial {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let distance = if self.0 > other.0 {
            self.0 - other.0
        } else {
            other.0 - self.0
        };
        if distance < u32::MAX / 2 {
            self.0.partial_cmp(&other.0)
        } else {
            // wrap-around occurred, invert comparison
            other.0.partial_cmp(&self.0)
        }
    }
}

impl From<u32> for Serial {
    fn from(n: u32) -> Self {
        Serial(n)
    }
}

impl From<Serial> for u32 {
    fn from(serial: Serial) -> u32 {
        serial.0
    }
}

/// A counter for generating serials, for use in the client protocol
///
/// A global instance of this counter is available as the `SERIAL_COUNTER`
/// static. It is recommended to only use this global counter to ensure the
/// uniqueness of serials.
///
/// The counter will wrap around on overflow, ensuring it can run for as long
/// as needed.
#[derive(Debug)]
pub struct SerialCounter {
    serial: AtomicU32,
}

impl SerialCounter {
    /// Retrieve the next serial from the counter
    pub fn next_serial(&self) -> Serial {
        Serial(self.serial.fetch_add(1, Ordering::AcqRel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_serial_counter(initial_value: u32) -> SerialCounter {
        SerialCounter {
            serial: AtomicU32::new(initial_value),
        }
    }

    #[test]
    #[allow(clippy::eq_op)]
    fn serial_equals_self() {
        let counter = create_serial_counter(0);
        let serial = counter.next_serial();
        assert!(serial == serial);
    }

    #[test]
    fn consecutive_serials() {
        let counter = create_serial_counter(0);
        let serial1 = counter.next_serial();
        let serial2 = counter.next_serial();
        assert!(serial1 < serial2);
    }

    #[test]
    fn non_consecutive_serials() {
        let skip_serials = 147;

        let counter = create_serial_counter(0);
        let serial1 = counter.next_serial();
        for _ in 0..skip_serials {
            let _ = counter.next_serial();
        }
        let serial2 = counter.next_serial();
        assert!(serial1 < serial2);
    }

    #[test]
    fn serial_wrap_around() {
        let counter = create_serial_counter(u32::MAX);
        let serial1 = counter.next_serial();
        let serial2 = counter.next_serial();

        assert!(serial1 == u32::MAX.into());
        assert!(serial2 == 0.into());

        assert!(serial1 < serial2);
    }
}
