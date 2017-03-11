#![warn(missing_docs)]

#![cfg_attr(feature = "clippy", feature(plugin))]
#![cfg_attr(feature = "clippy", plugin(clippy))]

#[macro_use]
extern crate wayland_server;
extern crate nix;

#[macro_use]
extern crate slog;
extern crate slog_stdlog;

pub mod shm;
