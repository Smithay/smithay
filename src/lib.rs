#![warn(missing_docs)]
//! # Smithay: the wayland composito smithy
//!
//! Most entry points in the modules can take an optionnal `slog::Logger` as argument
//! that will be used as a drain for logging. If `None` is provided, they'll log to `slog-stdlog`.


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

fn slog_or_stdlog<L>(logger: L) -> ::slog::Logger
    where L: Into<Option<::slog::Logger>>
{
    use slog::Drain;
    logger
        .into()
        .unwrap_or_else(|| ::slog::Logger::root(::slog_stdlog::StdLog.fuse(), o!()))
}
