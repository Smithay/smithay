#![cfg_attr(docsrs, feature(doc_auto_cfg))]
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
//! reference to a value, that will be passed down to most callbacks. This structure provides
//! easy access to a centralized mutable state without synchronization (as the callback invocation is
//! *always* sequential), and is the recommended way to of structuring your compositor.
//! TODO: Add a section here how this links to wayland-server's `Dispatch` and link to the wayland-server
//! docs, once they exist for 0.30.
//!
//! Several objects, in particular on the wayland clients side, can exist as multiple instances where each
//! instance has its own associated state. For these situations, these objects provide an interface allowing
//! you to associate an arbitrary value to them, that you can access at any time from the object itself
//! (rather than having your own container in which you search for the appropriate value when you need it).
//!
//! ### Logging
//!
//! TODO: docs about tracing

pub mod backend;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod input;
pub mod output;
pub mod utils;
#[cfg(feature = "wayland_frontend")]
pub mod wayland;

#[cfg(feature = "xwayland")]
pub mod xwayland;

pub mod reexports;
