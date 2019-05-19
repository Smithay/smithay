#![warn(missing_docs, rust_2018_idioms)]
//! **Smithay: the Wayland compositor smithy**
//!
//! Most entry points in the modules can take an optional [`slog::Logger`](::slog::Logger) as argument
//! that will be used as a drain for logging. If `None` is provided, they'll log to `slog-stdlog`.

// `error_chain!` can recurse deeply
#![recursion_limit = "1024"]

#[cfg_attr(feature = "backend_session", macro_use)]
#[doc(hidden)]
pub extern crate nix;
#[macro_use]
extern crate slog;
#[macro_use]
extern crate error_chain;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate bitflags;

pub mod backend;
pub mod utils;
#[cfg(feature = "wayland_frontend")]
pub mod wayland;

#[cfg(feature = "xwayland")]
pub mod xwayland;

pub mod reexports;

fn slog_or_stdlog<L>(logger: L) -> ::slog::Logger
where
    L: Into<Option<::slog::Logger>>,
{
    use slog::Drain;
    logger
        .into()
        .unwrap_or_else(|| ::slog::Logger::root(::slog_stdlog::StdLog.fuse(), o!()))
}
