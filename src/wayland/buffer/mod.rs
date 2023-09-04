//! Buffer management traits.
//!
//! This module provides the [`BufferHandler`] trait to notify compositors that a
//! [`WlBuffer`](wayland_server::protocol::wl_buffer::WlBuffer) managed by
//! Smithay has been destroyed.

use wayland_server::protocol::wl_buffer;

/// Handler trait for associating data with a [`WlBuffer`](wayland_server::protocol::wl_buffer::WlBuffer).
///
/// This trait primarily allows compositors to be told when a buffer is destroyed.
///
/// # For buffer abstractions
///
/// Buffer abstractions (such as [`shm`](crate::wayland::shm)) should require this trait in their
/// [`delegate_dispatch`](wayland_server::delegate_dispatch) implementations to notify the compositor when a
/// buffer is destroyed.
pub trait BufferHandler {
    /// Called when the client has destroyed the buffer.
    ///
    /// At this point the buffer is no longer usable by Smithay.
    fn buffer_destroyed(&mut self, buffer: &wl_buffer::WlBuffer);
}
