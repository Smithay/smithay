#![warn(missing_docs)]
//! **Smithay: the wayland compositor smithy**
//!
//! Most entry points in the modules can take an optionnal `slog::Logger` as argument
//! that will be used as a drain for logging. If `None` is provided, they'll log to `slog-stdlog`.

// `error_chain!` can recurse deeply
#![recursion_limit = "1024"]

extern crate image;
#[macro_use]
extern crate nix;
#[macro_use]
extern crate rental;
extern crate tempfile;
extern crate wayland_protocols;
extern crate wayland_server;
extern crate xkbcommon;

#[cfg(feature = "backend_drm")]
extern crate drm;
#[cfg(feature = "backend_drm")]
extern crate gbm;
#[cfg(feature = "backend_libinput")]
extern crate input;
#[cfg(feature = "udev")]
extern crate udev;
/*
#[cfg(feature = "backend_session_logind")]
extern crate dbus;
#[cfg(feature = "backend_session_logind")]
extern crate systemd;
*/
#[cfg(feature = "backend_winit")]
extern crate wayland_client;
#[cfg(feature = "backend_winit")]
extern crate winit;

extern crate libloading;

#[cfg(feature = "renderer_glium")]
extern crate glium;

#[macro_use]
extern crate slog;
extern crate slog_stdlog;

#[macro_use]
extern crate error_chain;

pub mod backend;
pub mod wayland;
pub mod utils;

fn slog_or_stdlog<L>(logger: L) -> ::slog::Logger
where
    L: Into<Option<::slog::Logger>>,
{
    use slog::Drain;
    logger
        .into()
        .unwrap_or_else(|| ::slog::Logger::root(::slog_stdlog::StdLog.fuse(), o!()))
}
