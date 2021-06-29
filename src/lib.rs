#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms)]
// Allow acronyms like EGL
#![allow(clippy::upper_case_acronyms)]

//! **Smithay: the Wayland compositor smithy**
//!
//! Most entry points in the modules can take an optional [`slog::Logger`](::slog::Logger) as argument
//! that will be used as a drain for logging. If `None` is provided, the behavior depends on
//! whether the `slog-stdlog` is enabled. If yes, the module will log to the global logger of the
//! `log` crate. If not, the logs will discarded. This cargo feature is part of the default set of
//! features of Smithay.

#[doc(hidden)]
pub extern crate nix;

pub mod backend;
pub mod utils;
#[cfg(feature = "wayland_frontend")]
pub mod wayland;

pub mod signaling;

#[cfg(feature = "xwayland")]
pub mod xwayland;

pub mod reexports;

#[cfg(feature = "slog-stdlog")]
#[allow(dead_code)]
fn slog_or_fallback<L>(logger: L) -> ::slog::Logger
where
    L: Into<Option<::slog::Logger>>,
{
    use slog::Drain;
    logger
        .into()
        .unwrap_or_else(|| ::slog::Logger::root(::slog_stdlog::StdLog.fuse(), slog::o!()))
}

#[cfg(not(feature = "slog-stdlog"))]
#[allow(dead_code)]
fn slog_or_fallback<L>(logger: L) -> ::slog::Logger
where
    L: Into<Option<::slog::Logger>>,
{
    logger
        .into()
        .unwrap_or_else(|| ::slog::Logger::root(::slog::Discard, slog::o!()))
}
