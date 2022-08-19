//! Handler utilities for the various shell protocols
//!
//! Wayland, via its different protocol extensions, supports different kind of
//! shells. Here a shell represent the logic associated to displaying windows and
//! arranging them on the screen.
//!
//! The shell protocols thus define what kind of interactions a client can have with
//! the compositor to properly display its contents on the screen.
//!
//! Smithay currently provides three of them:
//!
//! - The [`xdg`](xdg/index.html) module provides handlers for the `xdg_shell` protocol, which is
//!   the current standard for desktop apps
//! - The [`wlr_layer`](wlr_layer/index.html) module provides handlers for the `wlr_layer_shell`
//!   protocol, which is for windows rendering above/below normal XDG windows
//! - The [`kde`](kde/index.html) module provides handlers for KDE-specific protocols

use super::Serial;
use crate::wayland::compositor;
use thiserror::Error;
use wayland_server::protocol::wl_surface::WlSurface;

pub mod kde;
pub mod wlr_layer;
pub mod xdg;

/// Represents the possible errors returned from
/// a surface ping
#[derive(Debug, Error)]
pub enum PingError {
    /// The operation failed because the underlying surface has been destroyed
    #[error("the ping failed cause the underlying surface has been destroyed")]
    DeadSurface,
    /// There is already a pending ping
    #[error("there is already a ping pending `{0:?}`")]
    PingAlreadyPending(Serial),
}

/// Returns true if the surface is toplevel equivalent.
///
/// Currently is method only checks if the surface roles is `xdg_toplevel`,
/// but may be extended to other shell-protocols in the future, if applicable.
pub fn is_toplevel_equivalent(surface: &WlSurface) -> bool {
    // xdg_toplevel is toplevel like, so verify if the role matches.
    let role = compositor::get_role(surface);

    matches!(role, Some(xdg::XDG_TOPLEVEL_ROLE))
}
