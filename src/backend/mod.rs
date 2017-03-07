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

#[cfg(feature = "glutin")]
pub mod glutin;

#[cfg(feature = "glium")]
mod glium;
#[cfg(feature = "glium")]
pub use glium::*;

trait NewIdType {
    fn new(id: u32) -> Self;
}
