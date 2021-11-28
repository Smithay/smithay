#![warn(rust_2018_idioms)]
// If no backend is enabled, a large portion of the codebase is unused.
// So silence this useless warning for the CI.
#![cfg_attr(
    not(any(feature = "winit", feature = "udev")),
    allow(dead_code, unused_imports)
)]

#[macro_use]
extern crate slog;

#[cfg(feature = "udev")]
pub mod cursor;
pub mod drawing;
pub mod input_handler;
pub mod shell;
pub mod state;
#[cfg(feature = "udev")]
pub mod udev;
#[cfg(feature = "winit")]
pub mod winit;
#[cfg(feature = "x11")]
pub mod x11;
#[cfg(feature = "xwayland")]
pub mod xwayland;

pub use state::AnvilState;
