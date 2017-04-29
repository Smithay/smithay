//! Common traits and types used for software rendering on graphics backends


use super::GraphicsBackend;
use std::error::Error;
use wayland_server::protocol::wl_shm::Format;

/// Trait that describes objects providing a software rendering implementation
pub trait CpuGraphicsBackend<E: Error>: GraphicsBackend {
    /// Render a given buffer of a given format at a specified place in the framebuffer
    ///
    /// # Error
    /// Returns an error if the buffer size does not match the required amount of pixels
    /// for the given size or if the position and size is out of scope of the framebuffer.
    fn render(&mut self, buffer: &[u8], format: Format, at: (u32, u32), size: (u32, u32)) -> Result<(), E>;

    /// Returns the dimensions of the Framebuffer
    fn get_framebuffer_dimensions(&self) -> (u32, u32);
}
