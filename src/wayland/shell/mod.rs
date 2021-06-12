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

use super::Serial;
use thiserror::Error;

pub mod legacy;
pub mod xdg;

/// Represents the possible errors returned from
/// a surface operation
#[derive(Debug, Error)]
pub enum SurfaceError {
    /// The underlying `WlSurface` is no longer alive
    #[error("the surface is not alive")]
    SurfaceNotAlive,
}

/// Represents the possible errors returned from
/// a surface ping
#[derive(Debug, Error)]
pub enum PingError {
    /// The operation failed because the underlying surface has an error
    #[error("the ping failed cause the underlying surface has an error")]
    SurfaceError(#[from] SurfaceError),
    /// There is already a pending ping
    #[error("there is already a ping pending `{0:?}`")]
    PingAlreadyPending(Serial),
}
