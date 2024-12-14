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

use crate::{utils::Serial, wayland::compositor};
use thiserror::Error;
use wayland_server::protocol::wl_surface::WlSurface;
use xdg::XdgToplevelSurfaceData;

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

    // When changing this, don't forget to change the check in is_valid_parent() below.
    matches!(role, Some(xdg::XDG_TOPLEVEL_ROLE))
}

/// Returns true if the `parent` is valid to set for `child`.
///
/// This will check that `parent` is toplevel equivalent, then make sure that it doesn't introduce
/// a parent loop.
pub fn is_valid_parent(child: &WlSurface, parent: &WlSurface) -> bool {
    if !is_toplevel_equivalent(parent) {
        return false;
    }

    // Check that we're not making a parent loop.
    let mut next_parent = Some(parent.clone());
    while let Some(parent) = next_parent.clone() {
        // Did we find a cycle?
        if *child == parent {
            return false;
        }

        compositor::with_states(&parent, |states| {
            if let Some(data) = states.data_map.get::<XdgToplevelSurfaceData>() {
                // Get xdg-toplevel parent.
                let role = data.lock().unwrap();
                next_parent = role.parent.clone();
            } else {
                // Reached a surface we don't know how to get a parent of.
                next_parent = None;
            }
        });
    }

    true
}
