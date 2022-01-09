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
//! - The [`xdg`](xdg/index.html) module provides handlers for the `xdg_shell` protocol, which is
//!   the current standard for desktop apps
//! - The [`legacy`](legacy/index.html) module provides handlers for the `wl_shell` protocol, which
//!   is now deprecated. You only need it if you want to support apps predating `xdg_shell`.

use super::Serial;
use crate::wayland::compositor;
use thiserror::Error;
use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle};

// pub mod legacy;
pub mod xdg;

pub mod wlr_layer;

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
/// This is method checks if the surface roles is one of `wl_shell_surface`, `xdg_toplevel`
/// or `zxdg_toplevel`.
pub fn is_toplevel_equivalent(dh: &mut DisplayHandle<'_>, surface: &WlSurface) -> bool {
    // (z)xdg_toplevel and wl_shell_surface are toplevel like, so verify if the roles match.
    let role = compositor::get_role(dh, surface);

    matches!(
        role,
        Some(xdg::XDG_TOPLEVEL_ROLE) | Some(xdg::ZXDG_TOPLEVEL_ROLE) //| Some(legacy::WL_SHELL_SURFACE_ROLE)
    )
}
