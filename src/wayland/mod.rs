//! Protocol-related utilities
//!
//! This module contains several handlers to manage the wayland protocol
//! and the clients.
//!
//! Most utilities provided in this module work in the wame way:
//!
//! - An init function or method will take the event loop as argument and
//!   insert one or more globals into it.
//! - If you want to remove a previously inserted global, just call the
//!   `destroy()` method on the associated `Global`. If you don't plan to
//!   destroy the global at all, you don't need to bother keeping the
//!   `Global` around.

pub mod compositor;
pub mod output;
pub mod seat;
pub mod shell;
pub mod shm;
