//! Handler utilities for the various shell protocols
//!
//! Wayland, via its different protocol extensions, supports different kind of
//! shells. Here a shell represent the logic associated to displaying windows and
//! arranging them on the screen.
//!
//! The shell protocols thus define what kind of interactions a client can have with
//! the compositor to properly display its contents on the screen.
//!
//! Smithay currently provides two of them:
//!
//! - The [`xdg`](xdg/index.hmtl) module provides handlers for the `xdg_shell` protocol, which is
//!   the current standard for desktop apps
//! - The [`legacy`](legacy/index.html) module provides handlers for the `wl_shell` protocol, which
//!   is now deprecated. You only need it if you want to support apps predating `xdg_shell`.

pub mod legacy;
pub mod xdg;
