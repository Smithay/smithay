//! Backend (rendering/input) creation helpers
//!
//! Collection of common traits and implementation about
//! rendering onto various targets and receiving input
//! from various sources.
//!
//! Supported graphics backends:
//!
//! - glutin (headless/windowed)
//!
//! Supported input backends:
//!
//! - glutin (windowed)

pub mod input;
pub mod graphics;

#[cfg(feature = "backend_glutin")]
pub mod glutin;
#[cfg(feature = "backend_libinput")]
pub mod libinput;

#[cfg(feature = "renderer_glium")]
mod glium;
#[cfg(feature = "renderer_glium")]
pub use glium::*;

/// Internal functions that need to be accessible by the different backend implementations

trait SeatInternal {
    fn new(id: u32, capabilities: input::SeatCapabilities) -> Self;
}

trait TouchSlotInternal {
    fn new(id: u32) -> Self;
}
