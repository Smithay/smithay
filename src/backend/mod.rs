//! Backend (rendering/input) creation helpers
//!
//! Collection of common traits and implementation about
//! rendering onto various targets and receiving input
//! from various sources.
//!
//! Supported graphics backends:
//!
//! - winit
//!
//! Supported input backends:
//!
//! - winit
//! - libinput

pub mod input;
pub mod graphics;

#[cfg(feature = "backend_winit")]
pub mod winit;
#[cfg(feature = "backend_drm")]
pub mod drm;
#[cfg(feature = "backend_libinput")]
pub mod libinput;

// Internal functions that need to be accessible by the different backend implementations

trait SeatInternal {
    fn new(id: u64, capabilities: input::SeatCapabilities) -> Self;
    fn capabilities_mut(&mut self) -> &mut input::SeatCapabilities;
}

trait TouchSlotInternal {
    fn new(id: u64) -> Self;
}
