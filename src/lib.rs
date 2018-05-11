#![warn(missing_docs)]
//! **Smithay: the wayland compositor smithy**
//!
//! Most entry points in the modules can take an optionnal `slog::Logger` as argument
//! that will be used as a drain for logging. If `None` is provided, they'll log to `slog-stdlog`.

// `error_chain!` can recurse deeply
#![recursion_limit = "1024"]

pub extern crate image;
#[cfg_attr(feature = "backend_session", macro_use)]
extern crate nix;
extern crate tempfile;
pub extern crate wayland_protocols;
pub extern crate wayland_server;
extern crate wayland_sys;
extern crate xkbcommon;

#[cfg(feature = "dbus")]
pub extern crate dbus;
#[cfg(feature = "backend_drm")]
pub extern crate drm;
#[cfg(feature = "backend_drm")]
pub extern crate gbm;
#[cfg(feature = "backend_libinput")]
pub extern crate input;
#[cfg(feature = "backend_session_logind")]
pub extern crate systemd;
#[cfg(feature = "udev")]
pub extern crate udev;
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

#[macro_use]
extern crate lazy_static;

pub mod backend;
pub mod wayland;
pub mod utils;

#[cfg(feature = "xwayland")]
pub mod xwayland;

fn slog_or_stdlog<L>(logger: L) -> ::slog::Logger
where
    L: Into<Option<::slog::Logger>>,
{
    use slog::Drain;
    logger
        .into()
        .unwrap_or_else(|| ::slog::Logger::root(::slog_stdlog::StdLog.fuse(), o!()))
}
