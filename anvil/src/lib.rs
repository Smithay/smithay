#![warn(rust_2018_idioms)]

#[macro_use]
extern crate glium;
#[macro_use]
extern crate slog;
#[macro_use(define_roles)]
extern crate smithay;

#[macro_use]
pub mod shaders;
pub mod buffer_utils;
pub mod glium_drawer;
pub mod input_handler;
pub mod shell;
pub mod shm_load;
#[cfg(feature = "udev")]
pub mod udev;
pub mod window_map;
#[cfg(feature = "winit")]
pub mod winit;
