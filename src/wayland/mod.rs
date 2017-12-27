//! Protocol-related utilities
//!
//! This module contains several handlers to manage the wayland protocol
//! and the clients.
//!
//! Most utilities provided in this module work in the wame way:
//!
//! - An init function or method will take the event loop as argument and
//!   insert one or more globals into it.
//! - These functions will return the `Global` handles and, if applicable,
//!   a `StateToken` allowing you to access the associated state value in
//!   this event loop.
//! - If you want to remove a previously inserted global, just call the
//!   `destroy()` method on the associated `Global`. If you don't plan to
//!   destroy the global at all, you don't need to bother keeping the
//!   `Global` around.
//! - You should not remove a state value from the event loop if you have
//!   not previously destroyed all the globals using it, otherwise you'll
//!   quickly encounter a panic.

pub mod compositor;
pub mod output;
pub mod seat;
pub mod shm;
pub mod shell;
