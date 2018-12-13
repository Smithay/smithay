//! Protocol-related utilities
//!
//! This module contains several handlers to manage the Wayland protocol
//! and the clients.
//!
//! Most utilities provided in this module work in the same way:
//!
//! - An `init` function or method will take the event loop as argument and
//!   insert one or more globals into it.
//! - If you want to remove a previously inserted global, just call the
//!   `destroy()` method on the associated `Global`. If you don't plan to
//!   destroy the global at all, you don't need to bother keeping the
//!   `Global` around.

use std::sync::atomic::{AtomicUsize, Ordering};

pub mod compositor;
pub mod data_device;
pub mod output;
pub mod seat;
pub mod shell;
pub mod shm;

/// A global [`SerialCounter`] for use in your compositor.
///
/// Is is also used internally by some parts of Smithay.
pub static SERIAL_COUNTER: SerialCounter = SerialCounter {
    serial: AtomicUsize::new(0),
};

/// A counter for generating serials, for use in the client protocol
///
/// A global instance of this counter is available as the `SERIAL_COUNTER`
/// static. It is recommended to only use this global counter to ensure the
/// uniqueness of serials.
///
/// The counter will wrap around on overflow, ensuring it can run for as long
/// as needed.
pub struct SerialCounter {
    // TODO: replace with an AtomicU32 when stabilized
    serial: AtomicUsize,
}

impl SerialCounter {
    /// Retrieve the next serial from the counter
    pub fn next_serial(&self) -> u32 {
        self.serial.fetch_add(1, Ordering::AcqRel) as u32
    }
}
