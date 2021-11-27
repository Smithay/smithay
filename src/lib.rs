#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms)]
// Allow acronyms like EGL
#![allow(clippy::upper_case_acronyms)]

//! # Smithay: the Wayland compositor smithy
//!
//! This crate is a general framework for building wayland compositors. It currently focuses on low-level,
//! helpers and abstractions, handling most of the system-level and wayland protocol interactions.
//! The window management and drawing logic is however at the time not provided (but helpers for this
//! are planned for future version).
//!
//! ## Structure of the crate
//!
//! The provided helpers are split into two main modules. [`backend`] contains helpers for interacting with
//! the operating system, such as session management, interactions with the graphic stack and input
//! processing. On the other hand, [`wayland`] contains helpers for interacting with wayland clients
//! according to the wayland protocol. In addition, the [`xwayland`] module contains helpers for managing
//! an XWayland instance if you want to support it. See the documentation of these respective modules for
//! information about their usage.
//!
//! ## General principles for using Smithay
//!
//! ### The event loop and state handling
//!
//! Smithay is built around [`calloop`], a callback-oriented event loop, which fits naturally with the
//! general behavior of a wayland compositor: waiting for events to occur and react to them (be it
//! client requests, user input, or hardware events such as `vblank`).
//!
//! Using a callback-heavy structure however poses the question of state management: a lot of state needs
//! to be accessed from many different callbacks. To avoid an heavy requirement on shared pointers such
//! as `Rc` and `Arc` and the synchronization they require, [`calloop`] allows you to provide a mutable
//! reference to a value, that will be passed down to most callbacks (possibly under the form of a
//! [`DispatchData`](::wayland_server::DispatchData) for wayland-related callbacks). This structure provides
//! easy access to a centralized mutable state without synchronization (as the callback invocation is
//! *always* sequential), and is the recommended way to of structuring your compositor.
//!
//! Several objects, in particular on the wayland clients side, can exist as multiple instances where each
//! instance has its own associated state. For these situations, these objects provide an interface allowing
//! you to associate an arbitrary value to them, that you can access at any time from the object itself
//! (rather than having your own container in which you search for the appropriate value when you need it).
//!
//! ### Logging
//!
//! Most entry points in the modules can take an optional [`slog::Logger`](::slog::Logger) as argument
//! that will be used as a drain for logging. If `None` is provided, the behavior depends on
//! whether the `slog-stdlog` is enabled. If yes, the module will log to the global logger of the
//! `log` crate. If not, the logs will discarded. This cargo feature is part of the default set of
//! features of Smithay.

#[doc(hidden)]
pub extern crate nix;

pub mod backend;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod utils;
#[cfg(feature = "wayland_frontend")]
pub mod wayland;

// #[cfg(feature = "xwayland")]
// pub mod xwayland;

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
