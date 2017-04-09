#![warn(missing_docs)]

#![cfg_attr(feature = "clippy", feature(plugin))]
#![cfg_attr(feature = "clippy", plugin(clippy))]

#[macro_use]
extern crate wayland_server;
extern crate nix;
extern crate xkbcommon;
extern crate tempfile;

#[cfg(feature = "backend_glutin")]
extern crate glutin;

#[cfg(feature = "renderer_glium")]
extern crate glium;

#[macro_use]
extern crate slog;
extern crate slog_stdlog;

pub mod shm;
pub mod backend;
pub mod keyboard;
